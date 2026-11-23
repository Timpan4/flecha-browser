//! `open` and `save` serialize on `<name>.lock`. Each test holds the lock itself, starts the
//! operation on another thread, waits until the kernel lists that thread as blocked on the
//! lock, changes the file, and only then releases the lock.
#![cfg(target_os = "linux")]

use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::thread::{self, JoinHandle};

use flecha_permissions::{
    Decision, Partition, PermissionKind, PermissionManager, Snapshot, SnapshotError, SnapshotFile,
};

const VALID: &str = r#"{
  "partitions": {
    "shared": {
      "https://maps.example": {
        "geolocation": "block"
      }
    }
  },
  "version": 1
}
"#;

fn hold_lock(path: &Path) -> File {
    let lock = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_file_name("permissions.json.lock"))
        .unwrap();
    lock.lock().unwrap();
    lock
}

/// Returns once `/proc/locks` shows a waiter blocked on `lock`. Fails if the worker finishes
/// first, which means it did not wait for the lock.
fn wait_until_blocked<T>(lock: &File, worker: &JoinHandle<T>) {
    let waiter = format!(":{} ", lock.metadata().unwrap().ino());
    loop {
        assert!(!worker.is_finished(), "finished while the lock was held");
        let locks = fs::read_to_string("/proc/locks").unwrap();
        if locks
            .lines()
            .any(|line| line.contains("-> FLOCK") && line.contains(&waiter))
        {
            return;
        }
        thread::yield_now();
    }
}

#[test]
fn save_waits_for_the_lock_and_then_sees_a_newer_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.json");
    let (file, _) = SnapshotFile::open(&path).unwrap();
    let lock = hold_lock(&path);

    let saving = thread::spawn(move || file.save(&Snapshot::default()));
    wait_until_blocked(&lock, &saving);
    let newer = r#"{"version":2}"#;
    fs::write(&path, newer).unwrap();
    drop(lock);

    assert!(matches!(
        saving.join().unwrap(),
        Err(SnapshotError::NewerSchema(found)) if found == "2"
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), newer);
}

#[test]
fn recovery_never_moves_aside_a_file_saved_while_it_waited() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.json");
    fs::write(&path, "{ damaged").unwrap();
    let lock = hold_lock(&path);

    let opening = {
        let path = path.clone();
        thread::spawn(move || SnapshotFile::open(path))
    };
    wait_until_blocked(&lock, &opening);
    fs::write(&path, VALID).unwrap();
    drop(lock);

    let (_, loaded) = opening.join().unwrap().unwrap();
    assert!(loaded.damaged_backup.is_none());
    assert!(!dir.path().join("permissions.json.damaged").exists());
    let manager = PermissionManager::from_snapshot(loaded.snapshot);
    let site = flecha_permissions::Origin::parse("https://maps.example").unwrap();
    assert_eq!(
        manager
            .site_settings(&Partition::Shared, &site)
            .decision(PermissionKind::Geolocation),
        Decision::Block
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), VALID);
}
