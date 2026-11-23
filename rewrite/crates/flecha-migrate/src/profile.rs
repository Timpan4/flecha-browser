use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read as _};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Component, Path};

use anyhow::{Context as _, Result, bail, ensure};
use rustix::fd::OwnedFd;
use rustix::fs::{self as fs_at, AtFlags, Dir, FileType, Mode, OFlags, RenameFlags, Stat};
use rustix::io::Errno;
use rustix::process::{Pid, test_kill_process};
use tempfile::TempDir;

use crate::{ProfileInUse, SHELL_EXTENSION_ID};

pub(super) struct Source {
    root: OwnedFd,
    initial_root: Stat,
    database: OwnedFd,
}

impl Source {
    pub(super) fn open(user_data_dir: &Path, profile_name: &OsStr) -> Result<Self> {
        ensure!(
            Path::new(profile_name).file_name() == Some(profile_name),
            "the Chromium profile must be a single directory name"
        );
        let root = open_absolute_dir(user_data_dir, None).context("open Chromium user data dir")?;
        let initial_root = fs_at::fstat(&root)?;
        check_singleton(&root)?;
        let profile = open_dir(&root, profile_name).context("open selected Chromium profile")?;
        let extensions = open_dir(&profile, "Local Extension Settings")
            .context("open Chromium extension storage directory")?;
        let database = open_dir(&extensions, SHELL_EXTENSION_ID)
            .context("open Flecha shell extension database directory")?;
        Ok(Self {
            root,
            initial_root,
            database,
        })
    }

    pub(super) fn copy_database(&self, destination: &Path) -> Result<()> {
        let initial_db = fs_at::fstat(&self.database)?;
        let mut entries = Dir::read_from(&self.database)?;
        while let Some(entry) = entries.read() {
            let entry = entry.context("list Flecha shell extension database")?;
            let name = entry.file_name();
            let bytes = name.to_bytes();
            if !is_database_file(bytes) {
                continue;
            }
            // A preflight avoids opening a special file. fstat verifies what openat obtained.
            let before = fs_at::statat(&self.database, name, AtFlags::SYMLINK_NOFOLLOW)?;
            require_regular_file(&before, &initial_db)?;
            let file = fs_at::openat(
                &self.database,
                name,
                OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
                Mode::empty(),
            )?;
            let opened = fs_at::fstat(&file)?;
            require_regular_file(&opened, &initial_db)?;
            require_unchanged(&before, &opened)?;
            let mut file = File::from(file);
            let mut copy = File::create(destination.join(OsStr::from_bytes(bytes)))?;
            let copied = io::copy(&mut file, &mut copy).context("copy extension database file")?;
            ensure!(
                Some(copied) == before.st_size.try_into().ok(),
                "extension database file changed during migration"
            );
            require_unchanged(&before, &fs_at::fstat(&file)?)?;
        }
        check_singleton(&self.root)?;
        require_unchanged(&initial_db, &fs_at::fstat(&self.database)?)?;
        require_unchanged(&self.initial_root, &fs_at::fstat(&self.root)?)?;
        validate_manifest(destination)
    }
}

pub(super) struct Output {
    parent: OwnedFd,
    name: std::ffi::OsString,
    scratch: TempDir,
}

impl Output {
    pub(super) fn new(destination: &Path, source: &Source) -> Result<Self> {
        let name = destination
            .file_name()
            .context("missing new profile name")?;
        let parent_path = destination.parent().context("missing new profile parent")?;
        let parent = open_absolute_dir(parent_path, Some(&source.initial_root))
            .context("open new profile parent outside Chromium user data dir")?;
        match fs_at::statat(&parent, name, AtFlags::SYMLINK_NOFOLLOW) {
            Err(Errno::NOENT) => {}
            Err(error) => return Err(error).context("check new profile destination"),
            Ok(_) => bail!("new profile destination already exists"),
        }
        let scratch =
            tempfile::tempdir_in(parent_path).context("create private migration workspace")?;
        Ok(Self {
            parent,
            name: name.to_owned(),
            scratch,
        })
    }

    pub(super) fn scratch(&self) -> &Path {
        self.scratch.path()
    }

    pub(super) fn publish(self) -> Result<()> {
        let scratch_name = self.scratch().file_name().context("missing scratch name")?;
        let scratch = open_dir(&self.parent, scratch_name)?;
        fs_at::renameat_with(
            &scratch,
            "profile",
            &self.parent,
            &self.name,
            RenameFlags::NOREPLACE,
        )
        .context("publish migrated profile at an unused destination")?;
        Ok(())
    }
}

fn open_absolute_dir(path: &Path, excluded: Option<&Stat>) -> Result<OwnedFd> {
    ensure!(path.is_absolute(), "expected an absolute directory path");
    let mut dir = open_dir(fs_at::CWD, "/")?;
    if let Some(excluded) = excluded {
        ensure!(
            !same_file(&fs_at::fstat(&dir)?, excluded),
            "destination is inside the old profile"
        );
    }
    for component in path.components() {
        let Component::Normal(name) = component else {
            ensure!(
                component == Component::RootDir,
                "parent directory components are not allowed"
            );
            continue;
        };
        dir = open_dir(&dir, name)?;
        if let Some(excluded) = excluded {
            ensure!(
                !same_file(&fs_at::fstat(&dir)?, excluded),
                "destination is inside the old profile"
            );
        }
    }
    Ok(dir)
}

fn open_dir(parent: impl rustix::fd::AsFd, name: impl rustix::path::Arg) -> Result<OwnedFd> {
    Ok(fs_at::openat(
        parent,
        name,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?)
}

fn check_singleton(root: &OwnedFd) -> Result<()> {
    let lock = match fs_at::readlinkat(root, "SingletonLock", Vec::new()) {
        Ok(lock) => lock,
        Err(Errno::NOENT) => return Ok(()),
        Err(Errno::INVAL) => return Err(ProfileInUse.into()),
        Err(error) => return Err(error).context("check Chromium SingletonLock"),
    };
    // Chromium's lock is a deliberately dangling symlink to <hostname>-<pid>.
    let target = lock.to_bytes();
    let Some(separator) = target.iter().rposition(|byte| *byte == b'-') else {
        return Err(ProfileInUse.into());
    };
    let (host, pid) = (&target[..separator], &target[separator + 1..]);
    if host == rustix::system::uname().nodename().to_bytes()
        && let Some(pid) = std::str::from_utf8(pid)
            .ok()
            .and_then(|pid| pid.parse::<i32>().ok())
            .filter(|pid| *pid > 0)
            .and_then(Pid::from_raw)
        && test_kill_process(pid) == Err(Errno::SRCH)
    {
        return Ok(());
    }
    Err(ProfileInUse.into())
}

fn is_database_file(name: &[u8]) -> bool {
    let numbered = |s: &[u8]| !s.is_empty() && s.iter().all(u8::is_ascii_digit);
    name == b"CURRENT"
        || name.strip_prefix(b"MANIFEST-").is_some_and(numbered)
        || [b".log", b".ldb", b".sst"]
            .iter()
            .any(|suffix| name.strip_suffix(suffix.as_slice()).is_some_and(numbered))
}

fn require_regular_file(stat: &Stat, directory: &Stat) -> Result<()> {
    ensure!(
        FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
            && stat.st_nlink == 1
            && stat.st_dev == directory.st_dev,
        "extension database entry must be a regular file with one link on the database filesystem"
    );
    Ok(())
}

fn same_file(left: &Stat, right: &Stat) -> bool {
    left.st_dev == right.st_dev && left.st_ino == right.st_ino
}

fn require_unchanged(before: &Stat, after: &Stat) -> Result<()> {
    ensure!(
        same_file(before, after)
            && before.st_size == after.st_size
            && before.st_mtime == after.st_mtime
            && before.st_mtime_nsec == after.st_mtime_nsec
            && before.st_ctime == after.st_ctime
            && before.st_ctime_nsec == after.st_ctime_nsec,
        "Chromium profile changed during migration"
    );
    Ok(())
}

fn validate_manifest(database: &Path) -> Result<()> {
    // LevelDB resolves the name inside CURRENT as a path. Accept only FileNum's decimal spelling.
    let max_current = "MANIFEST-\n".len() + u64::MAX.ilog10() as usize + 1;
    let mut current = Vec::new();
    File::open(database.join("CURRENT"))
        .context("open extension LevelDB CURRENT")?
        .take((max_current + 1) as u64)
        .read_to_end(&mut current)?;
    let valid = current.len() <= max_current
        && current
            .strip_prefix(b"MANIFEST-")
            .and_then(|value| value.strip_suffix(b"\n"))
            .is_some_and(|number| !number.is_empty() && number.iter().all(u8::is_ascii_digit));
    ensure!(valid, "extension LevelDB CURRENT names an invalid manifest");
    Ok(())
}
