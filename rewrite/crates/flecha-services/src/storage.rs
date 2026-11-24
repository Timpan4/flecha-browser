use std::fs;
use std::io::{self, BufReader, Write as _};
use std::path::{Path, PathBuf};
#[cfg(any(test, feature = "test-support"))]
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result};
use flecha_core::{
    BrowserState,
    storage::{self, BackupWrite, LoadedState, Slot},
};
use serde_json::Value;

pub struct StateStore {
    directory: PathBuf,
    #[cfg(any(test, feature = "test-support"))]
    fail_next_sync: AtomicBool,
}

impl StateStore {
    pub fn open(directory: &Path) -> Result<(Self, LoadedState)> {
        fs::create_dir_all(directory).context("create session storage")?;
        let store = Self {
            directory: directory.to_path_buf(),
            #[cfg(any(test, feature = "test-support"))]
            fail_next_sync: AtomicBool::new(false),
        };
        let current = store.read(Slot::Current)?;
        let last_good = store.read(Slot::LastGood)?;
        Ok((store, storage::load(current.as_ref(), last_good.as_ref())?))
    }

    fn path(&self, slot: Slot) -> PathBuf {
        self.directory.join(match slot {
            Slot::Current => "state.json",
            Slot::LastGood => "last-good.json",
            Slot::Damaged => "damaged.json",
            Slot::BeforeImport => "before-import.json",
        })
    }

    fn read(&self, slot: Slot) -> Result<Option<Value>> {
        match fs::read(self.path(slot)) {
            Ok(bytes) => Ok(Some(match serde_json::from_slice(&bytes) {
                Ok(value) => value,
                // Keep the original bytes even if the outer JSON is corrupt.
                Err(_) => Value::Array(bytes.into_iter().map(Value::from).collect()),
            })),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error).context("read session snapshot"),
        }
    }

    pub fn has_before_import(&self) -> bool {
        !matches!(
            self.path(Slot::BeforeImport).symlink_metadata(),
            Err(error) if error.kind() == io::ErrorKind::NotFound
        )
    }

    pub fn read_before_import(&self) -> Result<Option<BrowserState>> {
        let file = match fs::File::open(self.path(Slot::BeforeImport)) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("Read the pre-import backup"),
        };
        let value =
            serde_json::from_reader(BufReader::new(file)).context("Read the pre-import backup")?;
        Ok(Some(
            storage::state_before_import(Some(&value))?
                .context("The pre-import backup is unreadable")?,
        ))
    }

    pub fn write(&self, write: &BackupWrite) -> Result<()> {
        self.replace_snapshot(write)?
            .context("Sync session storage")
    }

    /// An outer error leaves the old snapshot in place. An inner error means the
    /// new snapshot is in place but the directory entry could not be synced.
    pub fn replace_snapshot(&self, write: &BackupWrite) -> Result<io::Result<()>> {
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer(&mut file, write.value())?;
        file.flush()?;
        file.as_file().sync_all()?;
        #[cfg(unix)]
        let directory = fs::File::open(&self.directory)?;
        file.persist(self.path(write.slot()))
            .context("replace session snapshot")?;
        #[cfg(any(test, feature = "test-support"))]
        if self.fail_next_sync.swap(false, Ordering::Relaxed) {
            return Ok(Err(io::Error::other("injected directory sync failure")));
        }
        #[cfg(unix)]
        let synced = directory.sync_all();
        #[cfg(not(unix))]
        let synced = Ok(());
        Ok(synced)
    }

    /// Makes the next snapshot replace the saved one and then report that the directory entry
    /// could not be synced, so callers can check how they treat that error.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fail_next_directory_sync(&self) {
        self.fail_next_sync.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flecha_core::{SpaceId, WindowId};

    #[test]
    fn damaged_bytes_are_set_aside_before_restoring_the_last_good_file() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let (store, mut loaded) = StateStore::open(directory.path())?;
        let original = BrowserState::new(SpaceId::new("personal"), WindowId::new("initial"));
        store.write(&loaded.ready().unwrap().save_current(&original)?)?;
        let (_, mut loaded) = StateStore::open(directory.path())?;
        store.write(&loaded.ready().unwrap().started().unwrap())?;

        let broken = [0xff, b'{'];
        fs::write(store.path(Slot::Current), broken)?;
        let (_, mut loaded) = StateStore::open(directory.path())?;
        assert!(loaded.ready().is_none());
        let restored = loaded.apply_recovery(|write| store.write(write))?;
        assert_eq!(restored.state.as_ref(), Some(&original));
        assert_eq!(store.read(Slot::Damaged)?, Some(serde_json::json!(broken)));
        Ok(())
    }
}
