//! The saved downloads list: a round trip, and recovery shaped like `flecha_core::storage`.

use std::path::{Path, PathBuf};

use flecha_downloads::persist::{self, DecodeError, Recovery, SCHEMA_VERSION, Slot, SlotWrite};
use flecha_downloads::{
    DownloadList, InterruptReason, NewDownload, RowState, Snapshot, SnapshotState, State,
};
use serde_json::{Value, json};

fn add(list: &mut DownloadList, cef_id: u32, state: Option<SnapshotState>) {
    let target = PathBuf::from(format!("/home/me/Downloads/{cef_id}.bin"));
    list.begin(NewDownload {
        cef_id,
        url: format!("https://example.com/{cef_id}.bin"),
        target: target.clone(),
        started_at: 1_764_000_000_000 + i64::from(cef_id),
        warning: None,
        prompted: false,
    })
    .unwrap();
    if let Some(state) = state {
        list.update(
            cef_id,
            Snapshot {
                state,
                received: 10,
                total: 20,
                full_path: &target,
            },
        )
        .unwrap();
    }
}

fn stopped_list() -> DownloadList {
    let mut list = DownloadList::new();
    add(&mut list, 1, Some(SnapshotState::Complete));
    add(&mut list, 2, Some(SnapshotState::Cancelled));
    add(
        &mut list,
        3,
        Some(SnapshotState::Interrupted(InterruptReason::NetworkFailed)),
    );
    list.downloads_folder_changed(Path::new("/home/me/Pictures"));
    list
}

fn stored(list: &DownloadList) -> Value {
    Value::String(persist::encode(list).unwrap())
}

#[test]
fn a_saved_list_reads_back_the_same() {
    let list = stopped_list();
    assert_eq!(
        persist::decode(&persist::encode(&list).unwrap()).unwrap(),
        list
    );

    let mut loaded = persist::load(Some(&stored(&list)), None).unwrap();
    let ready = loaded.ready().expect("nothing to recover");
    assert_eq!(ready.list, list);
    assert_eq!(ready.recovered, None);
    assert_eq!(ready.save(&list).unwrap().value(), &stored(&list));
    // The first good start promotes it to last-good; the next start has nothing to add.
    let last_good = ready.started().expect("last-good write");
    assert_eq!(
        (last_good.slot(), last_good.value()),
        (Slot::LastGood, &stored(&list))
    );
    let mut again = persist::load(Some(&stored(&list)), Some(&stored(&list))).unwrap();
    assert_eq!(again.ready().unwrap().started(), None);
}

#[test]
fn the_schema_is_versioned_camel_case_json() {
    let json: Value = serde_json::from_str(&persist::encode(&stopped_list()).unwrap()).unwrap();
    assert_eq!(json["version"], json!(SCHEMA_VERSION));
    assert_eq!(json["nextId"], json!(3));
    assert_eq!(json["lastPromptFolder"], json!("/home/me/Pictures"));
    assert_eq!(
        json["downloads"][2],
        json!({
            "id": 3,
            "url": "https://example.com/3.bin",
            "path": "/home/me/Downloads/3.bin",
            "received": 10,
            "total": 20,
            "state": "interrupted",
            "interruptReason": "NETWORK_FAILED",
            "startedAt": 1_764_000_000_003_i64,
        })
    );
}

#[test]
fn downloads_running_at_shutdown_come_back_interrupted_and_need_a_retry() {
    let mut list = DownloadList::new();
    add(&mut list, 1, None);
    add(&mut list, 2, Some(SnapshotState::Paused));
    let restored = persist::decode(&persist::encode(&list).unwrap()).unwrap();
    for download in restored.iter() {
        assert_eq!(download.state(), State::Interrupted);
        assert_eq!(download.interrupt_reason(), Some(InterruptReason::Crash));
        let row = restored.row(download);
        assert_eq!((row.state, row.can_resume), (RowState::Interrupted, false));
        assert!(restored.resume(download.id()).is_err());
        assert_eq!(restored.retry(download.id()), Ok(download.url().into()));
    }
    // Ids keep counting from where they stopped.
    let mut restored = restored;
    let next = restored
        .begin(NewDownload {
            cef_id: 1,
            url: "https://example.com/new".into(),
            target: PathBuf::from("/home/me/Downloads/new"),
            started_at: 5,
            warning: None,
            prompted: false,
        })
        .unwrap();
    assert_eq!(next, 3);
}

fn recover(loaded: &mut persist::Loaded) -> Vec<SlotWrite> {
    assert!(loaded.ready().is_none(), "recovery writes come first");
    let mut writes = Vec::new();
    loaded
        .apply_recovery(|write| {
            writes.push(write.clone());
            Ok::<(), ()>(())
        })
        .unwrap();
    assert!(loaded.ready().is_some());
    writes
}

#[test]
fn a_damaged_list_is_set_aside_and_the_last_good_copy_takes_over() {
    let good = stored(&stopped_list());
    let damaged = [
        Value::String("{ not json".into()),
        Value::String(r#"{"version": 1, "nextId": 1, "downloads": [{"id": 1}]}"#.into()),
        Value::String(
            persist::encode(&stopped_list())
                .unwrap()
                .replace("\"id\": 2", "\"id\": 1"),
        ),
        Value::String(r#"{"version": 1, "nextId": 0, "downloads": [{"id": 5, "url": "", "path": "", "received": 0, "total": 0, "state": "complete", "startedAt": 0}]}"#.into()),
        // Raw bytes, as flecha-services keeps a file whose outer JSON is corrupt.
        json!([255, 123]),
    ];
    for current in damaged {
        let mut loaded = persist::load(Some(&current), Some(&good)).unwrap();
        let writes = recover(&mut loaded);
        assert_eq!(writes[0].slot(), Slot::Damaged);
        assert_eq!(writes[0].value(), &current);
        assert_eq!(
            (writes[1].slot(), writes[1].value()),
            (Slot::Current, &good)
        );
        let ready = loaded.ready().unwrap();
        assert_eq!(ready.recovered, Some(Recovery::LastGood));
        assert_eq!(ready.list, stopped_list());
    }
}

#[test]
fn without_a_readable_backup_the_list_starts_fresh() {
    let current = Value::String("{ not json".into());
    for last_good in [None, Some(Value::String("[]".into()))] {
        let mut loaded = persist::load(Some(&current), last_good.as_ref()).unwrap();
        let writes = recover(&mut loaded);
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].slot(), Slot::Damaged);
        let ready = loaded.ready().unwrap();
        assert_eq!(ready.recovered, Some(Recovery::Fresh));
        assert_eq!(ready.list, DownloadList::new());
    }
    let mut empty = persist::load(None, None).unwrap();
    let ready = empty.ready().unwrap();
    assert_eq!((ready.recovered, ready.list.iter().count()), (None, 0));
}

#[test]
fn a_failed_recovery_write_is_retried_before_saving() {
    let mut loaded = persist::load(Some(&Value::String("x".into())), None).unwrap();
    assert_eq!(
        loaded
            .apply_recovery(|_| Err::<(), _>("disk full"))
            .unwrap_err(),
        "disk full"
    );
    assert!(loaded.ready().is_none());
    let writes = recover(&mut loaded);
    assert_eq!(writes[0].slot(), Slot::Damaged);
}

#[test]
fn a_newer_schema_is_never_overwritten() {
    let newer = Value::String(json!({"version": SCHEMA_VERSION + 1, "anything": true}).to_string());
    assert!(matches!(
        persist::load(Some(&newer), None),
        Err(DecodeError::UnsupportedVersion(v)) if v == SCHEMA_VERSION + 1
    ));
    // A damaged current list with a newer backup starts empty but keeps the backup.
    let mut loaded = persist::load(Some(&Value::String("x".into())), Some(&newer)).unwrap();
    let writes = recover(&mut loaded);
    assert_eq!(writes.len(), 1);
    assert_eq!(
        loaded.ready().unwrap().recovered,
        Some(Recovery::UnsupportedBackup)
    );
    // A good current list doesn't replace a newer backup.
    let mut loaded = persist::load(Some(&stored(&stopped_list())), Some(&newer)).unwrap();
    assert_eq!(loaded.ready().unwrap().started(), None);
}
