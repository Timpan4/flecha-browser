use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result};
use flecha_core::storage::{self, BackupWrite, LoadedState, Slot};
use serde_json::Value;

pub struct StateStore(PathBuf);

impl StateStore {
    pub fn open(directory: &Path) -> Result<(Self, LoadedState)> {
        fs::create_dir_all(directory).context("create session storage")?;
        let store = Self(directory.to_path_buf());
        let current = store.read(Slot::Current)?;
        let last_good = store.read(Slot::LastGood)?;
        Ok((store, storage::load(current.as_ref(), last_good.as_ref())?))
    }

    fn path(&self, slot: Slot) -> PathBuf {
        self.0.join(match slot {
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

    pub fn write(&self, write: &BackupWrite) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&self.0)?;
        serde_json::to_writer(&mut file, write.value())?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(self.path(write.slot()))
            .context("replace session snapshot")?;
        #[cfg(unix)]
        fs::File::open(&self.0)?.sync_all()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use flecha_core::{BrowserState, SpaceId, WindowId};

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
