//! Changes the process's working directory, so it runs in a test binary of its own.

use std::env;
use std::fs;

use flecha_permissions::{Snapshot, SnapshotFile};

#[test]
fn a_relative_path_is_fixed_when_opened() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    env::set_current_dir(first.path()).unwrap();
    let expected = env::current_dir().unwrap().join("permissions.json");

    // The file does not exist yet.
    let (mut file, _) = SnapshotFile::open("permissions.json").unwrap();
    assert_eq!(file.path(), expected);

    env::set_current_dir(second.path()).unwrap();
    file.save(&Snapshot::default()).unwrap();
    assert!(expected.exists());
    assert!(!second.path().join("permissions.json").exists());
    assert!(!second.path().join("permissions.json.lock").exists());
    assert_eq!(fs::read_dir(second.path()).unwrap().count(), 0);
}
