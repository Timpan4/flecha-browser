use std::fs;

use flecha_permissions::{
    Decision, DecodeError, Evaluation, LoadError, Origin, Partition, PermissionKind,
    PermissionManager, SCHEMA_VERSION, SiteControl, Snapshot, SnapshotFile, TabId, Verdict,
};

fn origin(url: &str) -> Origin {
    Origin::parse(url).unwrap()
}

fn sample() -> PermissionManager {
    let mut manager = PermissionManager::new();
    manager
        .set_setting(
            &Partition::Shared,
            &origin("https://meet.example"),
            SiteControl::Media,
            Decision::Allow,
        )
        .unwrap();
    manager
        .set_setting(
            &Partition::Isolated("work".into()),
            &origin("https://maps.example"),
            PermissionKind::Geolocation,
            Decision::Block,
        )
        .unwrap();
    manager
        .set_setting(
            &Partition::Ephemeral("incognito".into()),
            &origin("https://secret.example"),
            PermissionKind::Camera,
            Decision::Allow,
        )
        .unwrap();
    manager
}

#[test]
fn snapshot_round_trips_through_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.json");
    let (file, loaded) = SnapshotFile::open(&path).unwrap();
    assert_eq!(loaded.snapshot, Snapshot::default());
    assert!(loaded.damaged_backup.is_none());

    let mut manager = sample();
    assert!(manager.is_dirty());
    file.save(&manager.snapshot()).unwrap();
    manager.mark_saved();
    assert!(!manager.is_dirty());

    let written = fs::read_to_string(&path).unwrap();
    assert_eq!(
        written,
        r#"{
  "partitions": {
    "isolated:work": {
      "https://maps.example": {
        "geolocation": "block"
      }
    },
    "shared": {
      "https://meet.example": {
        "camera": "allow",
        "microphone": "allow"
      }
    }
  },
  "version": 1
}
"#
    );
    assert!(!written.contains("secret.example"));

    let (_, loaded) = SnapshotFile::open(&path).unwrap();
    assert_eq!(loaded.skipped_entries, 0);
    assert_eq!(loaded.snapshot, manager.snapshot());
    let mut restored = PermissionManager::from_snapshot(loaded.snapshot);
    assert!(!restored.is_dirty());
    let tab = TabId::new("t");
    assert_eq!(
        restored.evaluate(
            &Partition::Shared,
            &tab,
            &origin("https://meet.example"),
            SiteControl::Media
        ),
        Evaluation::Resolved(Verdict::Allow)
    );
    assert_eq!(
        restored.evaluate(
            &Partition::Isolated("work".into()),
            &tab,
            &origin("https://maps.example"),
            PermissionKind::Geolocation
        ),
        Evaluation::Resolved(Verdict::Block)
    );
    assert!(matches!(
        restored.evaluate(
            &Partition::Ephemeral("incognito".into()),
            &tab,
            &origin("https://secret.example"),
            PermissionKind::Camera
        ),
        Evaluation::Prompt(_)
    ));
    let leftovers: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(leftovers, ["permissions.json"]);
}

#[test]
fn newer_schema_is_left_untouched_and_cannot_be_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.json");
    for newer in [
        format!(r#"{{"version":{},"partitions":{{}}}}"#, SCHEMA_VERSION + 1),
        r#"{"version":7,"origins":["a new shape"]}"#.to_owned(),
        r#"{"version":2.5}"#.to_owned(),
    ] {
        fs::write(&path, &newer).unwrap();
        match SnapshotFile::open(&path) {
            Err(LoadError::NewerSchema(_)) => {}
            other => panic!("expected a newer-schema refusal, got {other:?}"),
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), newer);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}

#[test]
fn damaged_file_moves_aside_and_loads_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.json");
    let backup = dir.path().join("permissions.json.damaged");
    for damaged in [
        b"{\"version\":1,\"partitions\":{".to_vec(),
        b"[]".to_vec(),
        b"{\"partitions\":{}}".to_vec(),
        b"{\"version\":0,\"partitions\":{}}".to_vec(),
        b"{\"version\":\"1\",\"partitions\":{}}".to_vec(),
        b"{\"version\":1,\"partitions\":[]}".to_vec(),
        vec![0xff, 0xfe, b'{'],
    ] {
        fs::write(&path, &damaged).unwrap();
        let (file, loaded) = SnapshotFile::open(&path).unwrap();
        assert_eq!(loaded.damaged_backup.as_deref(), Some(backup.as_path()));
        assert_eq!(loaded.snapshot, Snapshot::default());
        assert_eq!(fs::read(&backup).unwrap(), damaged);
        assert!(!path.exists());

        file.save(&sample().snapshot()).unwrap();
        let (_, reloaded) = SnapshotFile::open(&path).unwrap();
        assert_eq!(reloaded.snapshot, sample().snapshot());
        assert!(reloaded.damaged_backup.is_none());
        fs::remove_file(&path).unwrap();
    }
}

#[test]
fn unreadable_path_is_an_error_not_damage() {
    let dir = tempfile::tempdir().unwrap();
    // A directory where the file should be cannot be read as one.
    let path = dir.path().join("permissions.json");
    fs::create_dir(&path).unwrap();
    assert!(matches!(SnapshotFile::open(&path), Err(LoadError::Io(_))));
    assert!(path.is_dir());
    assert!(!dir.path().join("permissions.json.damaged").exists());
}

#[test]
fn invalid_entries_never_become_grants() {
    let json = r#"{
      "version": 1,
      "partitions": {
        "shared": {
          "https://ok.example": {"camera": "allow", "teleport": "allow", "microphone": "ask"},
          "http://evil.example": {"camera": "allow"},
          "https://Upper.Example": {"camera": "allow"},
          "https://ok.example/path": {"camera": "allow"},
          "data:text/html,x": {"camera": "allow"},
          "http://localhost:3000": {"geolocation": "block"},
          "https://shape.example": "allow"
        },
        "isolated:": {"https://a.example": {"camera": "allow"}},
        "ephemeral:x": {"https://a.example": {"camera": "allow"}},
        "isolated:work": []
      }
    }"#;
    let decoded = Snapshot::decode(json).unwrap();
    assert_eq!(decoded.skipped_entries, 10);
    let manager = PermissionManager::from_snapshot(decoded.snapshot);
    let ok = manager.site_settings(&Partition::Shared, &origin("https://ok.example"));
    assert_eq!(ok.decision(PermissionKind::Camera), Decision::Allow);
    assert_eq!(ok.decision(PermissionKind::Microphone), Decision::Ask);
    for insecure in ["http://evil.example", "https://upper.example"] {
        assert_eq!(
            manager.site_settings(&Partition::Shared, &origin(insecure)),
            Default::default(),
            "{insecure}"
        );
    }
    assert_eq!(
        manager
            .site_settings(&Partition::Shared, &origin("http://localhost:3000"))
            .decision(PermissionKind::Geolocation),
        Decision::Block
    );
}

#[test]
fn decode_reports_version_before_shape() {
    assert!(matches!(
        Snapshot::decode(r#"{"version":99,"partitions":"later"}"#),
        Err(DecodeError::NewerSchema(found)) if found == "99"
    ));
    assert!(matches!(
        Snapshot::decode(r#"{"version":-1,"partitions":{}}"#),
        Err(DecodeError::Damaged(_))
    ));
}
