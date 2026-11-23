//! The downloads list: every state transition, held downloads, and the rows the sidebar shows
//! (`chromium/shell/downloads.ts`, `chromium/shell/ui/app.ts`).

use std::path::{Path, PathBuf};

use flecha_downloads::{
    DownloadId, DownloadList, InterruptReason, ListError, NewDownload, RowState, Snapshot,
    SnapshotState, State, Warning,
};

const URL: &str = "https://example.com/files/a.bin";

fn begin(
    list: &mut DownloadList,
    cef_id: u32,
    started_at: i64,
    warning: Option<Warning>,
) -> DownloadId {
    list.begin(NewDownload {
        cef_id,
        url: URL.into(),
        target: PathBuf::from(format!("/dl/{cef_id}.bin")),
        started_at,
        warning,
        prompted: false,
    })
    .unwrap()
}

fn report(list: &mut DownloadList, cef_id: u32, state: SnapshotState) -> Result<(), ListError> {
    list.update(
        cef_id,
        Snapshot {
            state,
            received: 50,
            total: 100,
            full_path: Path::new(""),
        },
    )
}

fn one() -> (DownloadList, DownloadId) {
    let mut list = DownloadList::new();
    let id = begin(&mut list, 7, 1_000, None);
    (list, id)
}

fn interrupted(reason: InterruptReason) -> SnapshotState {
    SnapshotState::Interrupted(reason)
}

#[test]
fn progress_pause_resume_and_complete() {
    let (mut list, id) = one();
    report(&mut list, 7, SnapshotState::InProgress).unwrap();
    let row = list.row(list.get(id).unwrap());
    assert_eq!(
        (row.state, row.received, row.total),
        (RowState::Progressing, 50, 100)
    );
    assert_eq!(list.pause(id), Ok(7));
    assert!(list.resume(id).is_err());

    report(&mut list, 7, SnapshotState::Paused).unwrap();
    let row = list.row(list.get(id).unwrap());
    assert_eq!(
        (row.state, row.paused, row.can_resume),
        (RowState::Progressing, true, true)
    );
    assert_eq!(list.resume(id), Ok(7));
    assert!(list.pause(id).is_err());

    report(&mut list, 7, SnapshotState::InProgress).unwrap();
    report(&mut list, 7, SnapshotState::Complete).unwrap();
    assert_eq!(list.get(id).unwrap().state(), State::Complete);
    assert_eq!(list.row(list.get(id).unwrap()).state, RowState::Completed);
    // CEF may repeat the last report.
    report(&mut list, 7, SnapshotState::Complete).unwrap();
}

#[test]
fn a_paused_download_can_finish_stop_or_be_cancelled() {
    for (to, state) in [
        (SnapshotState::Complete, State::Complete),
        (SnapshotState::Cancelled, State::Cancelled),
        (
            interrupted(InterruptReason::NetworkFailed),
            State::Interrupted,
        ),
    ] {
        let (mut list, id) = one();
        report(&mut list, 7, SnapshotState::Paused).unwrap();
        report(&mut list, 7, to).unwrap();
        assert_eq!(list.get(id).unwrap().state(), state);
    }
}

#[test]
fn finished_and_cancelled_downloads_stay_that_way() {
    let later = [
        SnapshotState::InProgress,
        SnapshotState::Paused,
        interrupted(InterruptReason::NetworkFailed),
    ];
    for (end, state) in [
        (SnapshotState::Complete, State::Complete),
        (SnapshotState::Cancelled, State::Cancelled),
    ] {
        for next in later
            .iter()
            .chain([&SnapshotState::Complete, &SnapshotState::Cancelled])
        {
            let (mut list, id) = one();
            report(&mut list, 7, end).unwrap();
            if *next == end {
                continue;
            }
            assert!(
                matches!(
                    report(&mut list, 7, *next),
                    Err(ListError::Transition { .. })
                ),
                "{end:?} → {next:?}"
            );
            assert_eq!(list.get(id).unwrap().state(), state);
        }
    }
}

#[test]
fn an_interrupted_download_resumes_when_chromium_allows_it() {
    let (mut list, id) = one();
    report(&mut list, 7, interrupted(InterruptReason::NetworkFailed)).unwrap();
    let row = list.row(list.get(id).unwrap());
    assert_eq!((row.state, row.can_resume), (RowState::Interrupted, true));
    assert_eq!(list.resume(id), Ok(7));
    assert_eq!(list.retry(id), Ok(URL.into()));
    assert!(list.cancel(id).is_err());

    // Resuming, failing again, resuming and finishing.
    report(&mut list, 7, SnapshotState::InProgress).unwrap();
    report(
        &mut list,
        7,
        interrupted(InterruptReason::ServerContentLengthMismatch),
    )
    .unwrap();
    report(&mut list, 7, interrupted(InterruptReason::NetworkTimeout)).unwrap();
    assert_eq!(
        list.get(id).unwrap().interrupt_reason(),
        Some(InterruptReason::NetworkTimeout)
    );
    report(&mut list, 7, SnapshotState::InProgress).unwrap();
    assert_eq!(list.get(id).unwrap().interrupt_reason(), None);
    report(&mut list, 7, SnapshotState::Complete).unwrap();

    // It can't finish without resuming first.
    let (mut list, _) = one();
    report(&mut list, 7, interrupted(InterruptReason::NetworkFailed)).unwrap();
    assert!(matches!(
        report(&mut list, 7, SnapshotState::Complete),
        Err(ListError::Transition { .. })
    ));
    report(&mut list, 7, SnapshotState::Cancelled).unwrap();
}

#[test]
fn resumability_follows_get_download_resume_mode() {
    let resumable = [
        InterruptReason::FileFailed,
        InterruptReason::FileAccessDenied,
        InterruptReason::FileNoSpace,
        InterruptReason::FileNameTooLong,
        InterruptReason::FileTooLarge,
        InterruptReason::FileTransientError,
        InterruptReason::FileTooShort,
        InterruptReason::FileHashMismatch,
        InterruptReason::NetworkFailed,
        InterruptReason::NetworkTimeout,
        InterruptReason::NetworkDisconnected,
        InterruptReason::NetworkServerDown,
        InterruptReason::ServerFailed,
        InterruptReason::ServerNoRange,
        InterruptReason::ServerUnreachable,
        InterruptReason::ServerContentLengthMismatch,
        InterruptReason::Crash,
    ];
    for reason in InterruptReason::ALL {
        let (mut list, id) = one();
        report(&mut list, 7, interrupted(reason)).unwrap();
        let row = list.row(list.get(id).unwrap());
        if reason == InterruptReason::UserCanceled {
            assert_eq!(row.state, RowState::Cancelled);
            assert!(list.retry(id).is_err());
            continue;
        }
        assert_eq!(row.can_resume, resumable.contains(&reason), "{reason:?}");
        assert_eq!(list.resume(id).is_ok(), row.can_resume, "{reason:?}");
        assert_eq!(list.retry(id), Ok(URL.into()));
    }

    // Only HTTP(S) downloads resume.
    let mut list = DownloadList::new();
    let id = list
        .begin(NewDownload {
            cef_id: 1,
            url: "ftp://example.com/a.bin".into(),
            target: PathBuf::from("/dl/a.bin"),
            started_at: 1,
            warning: None,
            prompted: false,
        })
        .unwrap();
    report(&mut list, 1, interrupted(InterruptReason::NetworkFailed)).unwrap();
    assert!(!list.row(list.get(id).unwrap()).can_resume);
}

#[test]
fn snapshots_map_from_cef_flags_and_codes() {
    use SnapshotState as S;
    assert_eq!(
        S::from_cef(true, false, false, false, false, 0),
        Some(S::InProgress)
    );
    assert_eq!(
        S::from_cef(true, true, false, false, false, 0),
        Some(S::Paused)
    );
    assert_eq!(
        S::from_cef(false, false, true, false, false, 0),
        Some(S::Complete)
    );
    assert_eq!(
        S::from_cef(false, false, false, true, false, 40),
        Some(S::Cancelled)
    );
    assert_eq!(
        S::from_cef(false, false, false, false, true, 20),
        Some(S::Interrupted(InterruptReason::NetworkFailed))
    );
    assert_eq!(S::from_cef(false, false, false, false, true, 0), None);
    assert_eq!(S::from_cef(false, false, false, false, true, 32), None);
    assert_eq!(S::from_cef(true, false, true, false, false, 0), None);
    assert_eq!(S::from_cef(false, false, false, false, false, 0), None);
    for reason in InterruptReason::ALL {
        assert_eq!(InterruptReason::from_code(reason.code()), Some(reason));
    }
}

#[test]
fn a_held_download_is_kept_or_discarded() {
    let mut list = DownloadList::new();
    let kept = begin(&mut list, 1, 1_000, Some(Warning::Insecure));
    let discarded = begin(&mut list, 2, 2_000, Some(Warning::Dangerous));
    report(&mut list, 1, SnapshotState::InProgress).unwrap();
    assert_eq!(
        list.row(list.get(kept).unwrap()).warning,
        Some(Warning::Insecure)
    );
    // The sidebar offers only Keep and Discard for it.
    assert!(list.cancel(kept).is_err());
    assert!(list.pause(kept).is_err());
    assert!(matches!(
        report(&mut list, 1, SnapshotState::Complete),
        Err(ListError::CompletedWhileHeld(_))
    ));

    assert_eq!(list.keep(kept), Ok(Some(1)));
    assert_eq!(list.row(list.get(kept).unwrap()).warning, None);
    assert_eq!(list.keep(kept), Err(ListError::NotHeld(kept)));
    assert_eq!(list.discard(kept).unwrap_err(), ListError::NotHeld(kept));
    report(&mut list, 1, SnapshotState::Complete).unwrap();

    let (removed, cef_id) = list.discard(discarded).unwrap();
    assert_eq!((removed.id(), cef_id), (discarded, Some(2)));
    assert!(list.get(discarded).is_none());
    assert_eq!(list.keep(discarded), Err(ListError::NoDownload(discarded)));
    assert_eq!(
        report(&mut list, 2, SnapshotState::InProgress),
        Err(ListError::UnknownCefId(2))
    );
    assert_eq!(
        ListError::NotHeld(4).to_string(),
        "Download 4 isn't waiting for a decision."
    );
    assert_eq!(
        ListError::NoDownload(4).to_string(),
        "No download with id 4."
    );
}

#[test]
fn a_held_download_that_stops_is_no_longer_waiting() {
    let mut list = DownloadList::new();
    let id = begin(&mut list, 1, 1_000, Some(Warning::Dangerous));
    report(&mut list, 1, interrupted(InterruptReason::NetworkFailed)).unwrap();
    assert_eq!(list.row(list.get(id).unwrap()).warning, None);
    assert_eq!(list.keep(id), Err(ListError::NotHeld(id)));
    assert_eq!(list.discard(id).unwrap_err(), ListError::NotHeld(id));
}

#[test]
fn removing_drops_a_stopped_download_but_not_a_running_one() {
    let (mut list, id) = one();
    assert!(matches!(list.remove(id), Err(ListError::NotAllowed { .. })));
    report(&mut list, 7, SnapshotState::Complete).unwrap();
    assert_eq!(list.remove(id).unwrap().id(), id);
    assert!(list.get(id).is_none());
    assert_eq!(list.remove(id).unwrap_err(), ListError::NoDownload(id));
    assert_eq!(
        report(&mut list, 7, SnapshotState::Complete),
        Err(ListError::UnknownCefId(7))
    );
}

#[test]
fn a_cef_id_names_one_download_at_a_time() {
    let (mut list, _) = one();
    assert_eq!(
        list.begin(NewDownload {
            cef_id: 7,
            url: URL.into(),
            target: PathBuf::from("/dl/b.bin"),
            started_at: 2,
            warning: None,
            prompted: false,
        }),
        Err(ListError::CefIdInUse(7))
    );
    assert_eq!(
        report(&mut list, 8, SnapshotState::InProgress),
        Err(ListError::UnknownCefId(8))
    );
}

#[test]
fn clearing_keeps_progressing_and_held_rows_and_older_sessions() {
    let mut list = DownloadList::new();
    let older = begin(&mut list, 1, 100, None);
    report(&mut list, 1, SnapshotState::Complete).unwrap();
    let complete = begin(&mut list, 2, 1_001, None);
    report(&mut list, 2, SnapshotState::Complete).unwrap();
    let cancelled = begin(&mut list, 3, 1_002, None);
    report(&mut list, 3, SnapshotState::Cancelled).unwrap();
    let failed = begin(&mut list, 4, 1_003, None);
    report(&mut list, 4, interrupted(InterruptReason::ServerForbidden)).unwrap();
    let running = begin(&mut list, 5, 1_004, None);
    let paused = begin(&mut list, 6, 1_005, None);
    report(&mut list, 6, SnapshotState::Paused).unwrap();
    let held = begin(&mut list, 7, 1_006, Some(Warning::Insecure));

    let mut cleared = list.clear(1_000);
    cleared.sort();
    assert_eq!(cleared, vec![complete, cancelled, failed]);
    let left: Vec<_> = list.iter().map(|d| d.id()).collect();
    assert_eq!(left, vec![older, running, paused, held]);
}

#[test]
fn rows_show_this_session_newest_first() {
    let mut list = DownloadList::new();
    begin(&mut list, 1, 500, None);
    let first = begin(&mut list, 2, 1_500, None);
    let second = begin(&mut list, 3, 2_500, None);
    let tied = begin(&mut list, 4, 2_500, None);
    let rows: Vec<_> = list.rows(1_000).iter().map(|row| row.id).collect();
    assert_eq!(rows, vec![tied, second, first]);
}

#[test]
fn a_dialog_answer_becomes_the_next_dialogs_folder() {
    let mut list = DownloadList::new();
    list.begin(NewDownload {
        cef_id: 1,
        url: URL.into(),
        target: PathBuf::from("/home/me/Downloads/a.png"),
        started_at: 1,
        warning: None,
        prompted: true,
    })
    .unwrap();
    assert_eq!(list.last_prompt_folder(), None);
    list.update(
        1,
        Snapshot {
            state: SnapshotState::InProgress,
            received: 0,
            total: -1,
            full_path: Path::new("/home/me/Pictures/a.png"),
        },
    )
    .unwrap();
    assert_eq!(
        list.last_prompt_folder(),
        Some(Path::new("/home/me/Pictures"))
    );
    assert_eq!(list.row(list.iter().next().unwrap()).total, 0);
    list.downloads_folder_changed(Path::new("/media/usb"));
    assert_eq!(list.last_prompt_folder(), Some(Path::new("/media/usb")));
}

fn row_for_path(path: &str, url: &str) -> flecha_downloads::DownloadRow {
    let mut list = DownloadList::new();
    let id = list
        .begin(NewDownload {
            cef_id: 1,
            url: url.into(),
            target: PathBuf::from(path),
            started_at: 1,
            warning: None,
            prompted: false,
        })
        .unwrap();
    list.row(list.get(id).unwrap())
}

/// Ported from `chromium/tests/downloads.test.mjs`.
#[test]
fn a_download_is_listed_by_its_file_name_on_linux_macos_and_windows() {
    let get = "https://example.com/get";
    assert_eq!(
        row_for_path("/home/me/Downloads/report.pdf", get).filename,
        "report.pdf"
    );
    assert_eq!(
        row_for_path("C:\\Users\\me\\Downloads\\report.pdf", get).filename,
        "report.pdf"
    );
    assert_eq!(
        row_for_path("\\\\server\\share\\report.pdf", get).filename,
        "report.pdf"
    );
    assert_eq!(
        row_for_path("C:/Users/me\\Downloads\\report.pdf", get).filename,
        "report.pdf"
    );
    assert_eq!(
        row_for_path("C:\\Users\\me\\Downloads\\report.pdf", get).path,
        "C:\\Users\\me\\Downloads\\report.pdf"
    );
    // Elsewhere a backslash can be part of the name.
    assert_eq!(
        row_for_path("/home/me/Downloads/a\\b.txt", get).filename,
        "a\\b.txt"
    );
}

#[test]
fn without_a_path_the_row_is_named_from_the_url() {
    assert_eq!(row_for_path("", "https://example.com/get").filename, "get");
    assert_eq!(
        row_for_path("", "https://example.com/").filename,
        "download"
    );
    assert_eq!(row_for_path("", "not a url").filename, "download");
}
