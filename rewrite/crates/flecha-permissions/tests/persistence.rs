use std::fs;
use std::path::Path;

use flecha_permissions::{
    Decision, DecodeError, Evaluation, Origin, Partition, PermissionKind, PermissionManager,
    SCHEMA_VERSION, SiteControl, Snapshot, SnapshotError, SnapshotFile, TabId, Verdict,
};

fn origin(url: &str) -> Origin {
    Origin::parse(url).unwrap()
}

/// The directory's file names, sorted.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
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
    assert_eq!(
        entries(dir.path()),
        ["permissions.json", "permissions.json.lock"]
    );
}

#[test]
fn newer_schema_is_left_untouched_and_cannot_be_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.json");
    for newer in [
        format!(r#"{{"version":{},"partitions":{{}}}}"#, SCHEMA_VERSION + 1),
        r#"{"version":7,"origins":["a new shape"]}"#.to_owned(),
        r#"{"version":2.5}"#.to_owned(),
        // The version decides before the payload is decoded, so a payload this build cannot
        // parse, or an overflowing version, still reads as newer.
        r#"{"version":2,"future_payload":1e400}"#.to_owned(),
        r#"{"version":2e400,"partitions":{}}"#.to_owned(),
        r#"{"version":"2","partitions":{}}"#.to_owned(),
    ] {
        fs::write(&path, &newer).unwrap();
        match SnapshotFile::open(&path) {
            Err(SnapshotError::NewerSchema(_)) => {}
            other => panic!("expected a newer-schema refusal, got {other:?}"),
        }
        assert_eq!(fs::read_to_string(&path).unwrap(), newer);
        assert_eq!(
            entries(dir.path()),
            ["permissions.json", "permissions.json.lock"]
        );
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
        b"{\"version\":\"one\",\"partitions\":{}}".to_vec(),
        b"{\"version\":1,\"partitions\":{},\"extra\":1e400}".to_vec(),
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
    assert!(matches!(
        SnapshotFile::open(&path),
        Err(SnapshotError::Io(_))
    ));
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
        "ephemeral:x": {"https://a.example": {"camera": "allow"}},
        "isolated:work": []
      }
    }"#;
    let decoded = Snapshot::decode(json).unwrap();
    assert_eq!(decoded.skipped_entries, 9);
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

#[test]
fn a_current_version_spelled_as_a_string_still_loads() {
    let decoded = Snapshot::decode(r#"{"version":"1","partitions":{}}"#).unwrap();
    assert_eq!(decoded.snapshot, Snapshot::default());
}

#[test]
fn save_refuses_a_newer_file_written_after_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.json");
    let (file, _) = SnapshotFile::open(&path).unwrap();
    let newer = r#"{"version":2,"future_payload":1e400}"#;
    fs::write(&path, newer).unwrap();

    assert!(matches!(
        file.save(&sample().snapshot()),
        Err(SnapshotError::NewerSchema(found)) if found == "2"
    ));
    assert_eq!(fs::read_to_string(&path).unwrap(), newer);
    assert_eq!(
        entries(dir.path()),
        ["permissions.json", "permissions.json.lock"]
    );
}

#[test]
fn every_isolated_space_id_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.json");
    let site = origin("https://maps.example");
    let mut manager = PermissionManager::new();
    for id in ["", ":", "work"] {
        manager
            .set_setting(
                &Partition::Isolated(id.into()),
                &site,
                PermissionKind::Geolocation,
                Decision::Block,
            )
            .unwrap();
    }
    let (file, _) = SnapshotFile::open(&path).unwrap();
    file.save(&manager.snapshot()).unwrap();

    let (_, loaded) = SnapshotFile::open(&path).unwrap();
    assert_eq!(loaded.skipped_entries, 0);
    let mut restored = PermissionManager::from_snapshot(loaded.snapshot);
    for id in ["", ":", "work"] {
        assert_eq!(
            restored.evaluate(
                &Partition::Isolated(id.into()),
                &TabId::new("t"),
                &site,
                PermissionKind::Geolocation
            ),
            Evaluation::Resolved(Verdict::Block),
            "isolated {id:?}"
        );
    }
}

#[test]
fn a_failed_save_removes_only_its_own_temp_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("permissions.json");
    let (file, _) = SnapshotFile::open(&path).unwrap();
    // Files that look like temp files from another writer.
    let foreign = [
        dir.path().join(".permissions.json.abc123.tmp"),
        dir.path().join("permissions.json.1-2-0.tmp"),
    ];
    for other in &foreign {
        fs::write(other, "someone else's").unwrap();
    }
    // A non-empty directory at the destination makes the save fail.
    fs::create_dir(&path).unwrap();
    fs::write(path.join("keep"), "kept").unwrap();

    assert!(matches!(
        file.save(&sample().snapshot()),
        Err(SnapshotError::Io(_))
    ));
    for other in &foreign {
        assert_eq!(fs::read_to_string(other).unwrap(), "someone else's");
    }
    assert_eq!(fs::read_to_string(path.join("keep")).unwrap(), "kept");
    assert_eq!(
        entries(dir.path()),
        [
            ".permissions.json.abc123.tmp",
            "permissions.json",
            "permissions.json.1-2-0.tmp",
            "permissions.json.lock"
        ]
    );
}

#[test]
fn encoded_keys_sort_by_their_text() {
    let mut manager = PermissionManager::new();
    let site = origin("https://maps.example");
    for (partition, kind) in [
        (Partition::Shared, PermissionKind::Geolocation),
        (Partition::Shared, PermissionKind::ClipboardRead),
        (
            Partition::Isolated("a".into()),
            PermissionKind::Notifications,
        ),
    ] {
        manager
            .set_setting(&partition, &site, kind, Decision::Allow)
            .unwrap();
    }
    assert_eq!(
        manager.snapshot().encode(),
        r#"{
  "partitions": {
    "isolated:a": {
      "https://maps.example": {
        "notifications": "allow"
      }
    },
    "shared": {
      "https://maps.example": {
        "clipboard-read": "allow",
        "geolocation": "allow"
      }
    }
  },
  "version": 1
}
"#
    );
}
