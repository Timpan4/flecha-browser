use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufReader, Read as _};
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Component, Path};

use anyhow::{Context as _, Result, bail, ensure};
use rustix::fd::OwnedFd;
use rustix::fs::{self as fs_at, AtFlags, Dir, FileType, Mode, OFlags, RenameFlags, Stat};
use rustix::io::Errno;
use rustix::process::{Pid, test_kill_process};
use serde_json::Value;
use tempfile::TempDir;

use crate::{MigrationReport, ProfileInUse, SHELL_EXTENSION_ID};

pub(super) struct Source {
    root: OwnedFd,
    initial_root: Stat,
    profile: OwnedFd,
    initial_profile: Stat,
    database: OwnedFd,
    initial_db: Stat,
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
        let initial_profile = fs_at::fstat(&profile)?;
        let extensions = open_dir(&profile, "Local Extension Settings")
            .context("open Chromium extension storage directory")?;
        let database = open_dir(&extensions, SHELL_EXTENSION_ID)
            .context("open Flecha shell extension database directory")?;
        let initial_db = fs_at::fstat(&database)?;
        Ok(Self {
            root,
            initial_root,
            profile,
            initial_profile,
            database,
            initial_db,
        })
    }

    pub(super) fn copy_database(&self, destination: &Path) -> Result<()> {
        let mut entries = Dir::read_from(&self.database)?;
        while let Some(entry) = entries.read() {
            let entry = entry.context("list Flecha shell extension database")?;
            let name = entry.file_name();
            let bytes = name.to_bytes();
            if !is_database_file(bytes) {
                continue;
            }
            copy_file(
                &self.database,
                name,
                &self.initial_db,
                &fs_at::statat(&self.database, name, AtFlags::SYMLINK_NOFOLLOW)?,
                &destination.join(OsStr::from_bytes(bytes)),
            )?;
        }
        self.verify_unchanged()?;
        validate_manifest(destination)
    }

    pub(super) fn copy_browser_data(&self, cef: &Path, report: &mut MigrationReport) -> Result<()> {
        let default = cef.join("Default");
        report.cookies_copied = self.copy_cookies(&default)?;
        for name in [
            "Local Storage",
            "LocalStorage",
            "Session Storage",
            "SessionStorage",
        ] {
            report.web_storage_copied |= copy_storage(&self.profile, name, &default, false)
                .with_context(|| format!("copy Chromium {name}"))?;
        }
        for name in ["IndexedDB", "WebStorage"] {
            report.web_storage_copied |= copy_storage(&self.profile, name, &default, true)
                .with_context(|| format!("copy Chromium {name}"))?;
        }
        report.os_crypt_copied = self
            .copy_os_crypt(cef)
            .context("copy Chromium OSCrypt settings")?;
        Ok(())
    }

    fn copy_cookies(&self, destination: &Path) -> Result<bool> {
        if copy_cookie_database(&self.profile, destination).context("copy Chromium Cookies")? {
            return Ok(true);
        }
        let Some(network) = stat_if_present(&self.profile, "Network")? else {
            return Ok(false);
        };
        let directory = open_source_dir(&self.profile, "Network", &self.initial_profile, &network)
            .context("open Chromium Network directory")?;
        let copied = copy_cookie_database(&directory, destination)
            .context("copy Chromium Network Cookies")?;
        require_unchanged(&network, &fs_at::fstat(&directory)?)?;
        Ok(copied)
    }

    fn copy_os_crypt(&self, destination: &Path) -> Result<bool> {
        let Some(before) = stat_if_present(&self.root, "Local State")? else {
            return Ok(false);
        };
        let file = open_file(&self.root, "Local State", &self.initial_root, &before)?;
        let state: serde_json::Map<String, Value> = serde_json::from_reader(BufReader::new(&file))
            .map_err(|_| anyhow::anyhow!("invalid Chromium Local State JSON"))?;
        require_unchanged(&before, &fs_at::fstat(&file)?)?;
        let Some(os_crypt) = state.get("os_crypt") else {
            return Ok(false);
        };
        ensure!(os_crypt.is_object(), "Chromium os_crypt must be an object");
        std::fs::create_dir_all(destination)?;
        serde_json::to_writer(
            File::create(destination.join("Local State"))?,
            &serde_json::json!({ "os_crypt": os_crypt }),
        )
        .map_err(|_| anyhow::anyhow!("write Chromium OSCrypt settings"))?;
        Ok(true)
    }

    pub(super) fn verify_unchanged(&self) -> Result<()> {
        check_singleton(&self.root)?;
        require_unchanged(&self.initial_db, &fs_at::fstat(&self.database)?)?;
        require_unchanged(&self.initial_profile, &fs_at::fstat(&self.profile)?)?;
        require_unchanged(&self.initial_root, &fs_at::fstat(&self.root)?)
    }
}

fn copy_cookie_database(source: &OwnedFd, destination: &Path) -> Result<bool> {
    let Some(cookies) = stat_if_present(source, "Cookies")? else {
        return Ok(false);
    };
    let directory = fs_at::fstat(source)?;
    let files = [
        ("Cookies", Some(cookies)),
        ("Cookies-wal", stat_if_present(source, "Cookies-wal")?),
        ("Cookies-shm", stat_if_present(source, "Cookies-shm")?),
    ];
    std::fs::create_dir_all(destination)?;
    for (name, before) in &files {
        if let Some(before) = before {
            copy_file(source, *name, &directory, before, &destination.join(name))?;
        }
    }
    for (name, before) in files {
        if let Some(before) = before {
            require_unchanged(
                &before,
                &fs_at::statat(source, name, AtFlags::SYMLINK_NOFOLLOW)?,
            )?;
        }
    }
    require_unchanged(&directory, &fs_at::fstat(source)?)?;
    Ok(true)
}

fn copy_storage(
    source: &OwnedFd,
    name: &str,
    destination: &Path,
    directory_only: bool,
) -> Result<bool> {
    let Some(before) = stat_if_present(source, name)? else {
        return Ok(false);
    };
    std::fs::create_dir_all(destination)?;
    let directory = fs_at::fstat(source)?;
    let destination = destination.join(name);
    if directory_only || FileType::from_raw_mode(before.st_mode) == FileType::Directory {
        let child = open_source_dir(source, name, &directory, &before)?;
        copy_tree(&child, &before, &destination)?;
    } else {
        copy_file(source, name, &directory, &before, &destination)?;
    }
    Ok(true)
}

fn copy_tree(source: &OwnedFd, directory: &Stat, destination: &Path) -> Result<()> {
    std::fs::create_dir(destination)?;
    let mut entries = Dir::read_from(source)?;
    while let Some(entry) = entries.read() {
        let entry = entry?;
        let name = entry.file_name();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        let before = fs_at::statat(source, name, AtFlags::SYMLINK_NOFOLLOW)?;
        let destination = destination.join(OsStr::from_bytes(name.to_bytes()));
        if FileType::from_raw_mode(before.st_mode) == FileType::Directory {
            let child = open_source_dir(source, name, directory, &before)?;
            copy_tree(&child, &before, &destination)?;
        } else {
            copy_file(source, name, directory, &before, &destination)?;
        }
    }
    require_unchanged(directory, &fs_at::fstat(source)?)
}

fn copy_file(
    source: &OwnedFd,
    name: impl rustix::path::Arg,
    directory: &Stat,
    before: &Stat,
    destination: &Path,
) -> Result<()> {
    let mut file = open_file(source, name, directory, before)?;
    let copied = io::copy(&mut file, &mut File::create(destination)?)?;
    ensure!(
        Some(copied) == before.st_size.try_into().ok(),
        "Chromium file changed during migration"
    );
    require_unchanged(before, &fs_at::fstat(&file)?)
}

fn open_file(
    source: &OwnedFd,
    name: impl rustix::path::Arg,
    directory: &Stat,
    before: &Stat,
) -> Result<File> {
    // Check the entry before opening it and verify the descriptor obtained by openat.
    require_regular_file(before, directory)?;
    let file = fs_at::openat(
        source,
        name,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    let opened = fs_at::fstat(&file)?;
    require_regular_file(&opened, directory)?;
    require_unchanged(before, &opened)?;
    Ok(File::from(file))
}

fn open_source_dir(
    source: &OwnedFd,
    name: impl rustix::path::Arg,
    parent: &Stat,
    before: &Stat,
) -> Result<OwnedFd> {
    ensure!(
        FileType::from_raw_mode(before.st_mode) == FileType::Directory
            && before.st_dev == parent.st_dev,
        "Chromium storage entry must be a directory on the profile filesystem"
    );
    let child = open_dir(source, name)?;
    require_unchanged(before, &fs_at::fstat(&child)?)?;
    Ok(child)
}

fn stat_if_present(directory: &OwnedFd, name: &str) -> Result<Option<Stat>> {
    match fs_at::statat(directory, name, AtFlags::SYMLINK_NOFOLLOW) {
        Err(Errno::NOENT) => Ok(None),
        result => Ok(Some(result?)),
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
        "Chromium entry must be a regular file with one link on the parent filesystem"
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
