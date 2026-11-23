//! Where a download goes: the folder and ask-where-to-save settings, collisions and long
//! names, following `DownloadTargetDeterminer` and `download_path_reservation_tracker.cc`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use chrono::{NaiveDate, NaiveDateTime};
use flecha_core::Settings;
use flecha_downloads::{
    AskReason, DangerSignals, DownloadList, DownloadRequest, DownloadSettings, FolderError,
    InsecureSignals, MAX_UNIQUE_FILES, NameSources, NewDownload, Platform, Snapshot, SnapshotState,
    TargetDecision, TargetRequest, Trigger, decide, decide_target, generate_file_name,
    set_downloads_folder,
};
use url::Url;

/// ext4's documented file name limit, 255 bytes (kernel.org ext4 "Overview": "Max filename
/// length 255 bytes"). Real callers read it with `name_max`.
const EXT4_NAME_MAX: usize = 255;
/// NTFS's limit, 255 UTF-16 units (Microsoft "Maximum Path Length Limitation").
const NTFS_NAME_MAX: usize = 255;
const FOLDER: &str = "/home/me/Downloads";

fn settings(ask: bool) -> DownloadSettings {
    DownloadSettings {
        folder: PathBuf::from(FOLDER),
        ask_where_to_save: ask,
    }
}

fn started() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 11, 23)
        .unwrap()
        .and_hms_milli_opt(10, 15, 30, 123)
        .unwrap()
}

struct Case<'a> {
    name: &'a str,
    ask: bool,
    save_as: bool,
    last_prompt_folder: Option<&'a Path>,
    max: Option<usize>,
    platform: Platform,
}

impl Default for Case<'_> {
    fn default() -> Self {
        Self {
            name: "report.pdf",
            ask: false,
            save_as: false,
            last_prompt_folder: None,
            max: Some(EXT4_NAME_MAX),
            platform: Platform::Linux,
        }
    }
}

fn target(case: Case<'_>, in_use: impl Fn(&Path) -> bool) -> TargetDecision {
    let name = generate_file_name(
        NameSources {
            url: "https://example.com/",
            content_disposition: None,
            suggested_name: Some(case.name),
        },
        case.platform,
    );
    let settings = settings(case.ask);
    decide_target(
        TargetRequest {
            name: &name,
            settings: &settings,
            last_prompt_folder: case.last_prompt_folder,
            save_as: case.save_as,
            max_name_length: case.max,
            started: started(),
            platform: case.platform,
        },
        in_use,
    )
}

fn free(_: &Path) -> bool {
    false
}

fn saved(decision: TargetDecision) -> PathBuf {
    match decision {
        TargetDecision::Save(path) => path,
        other => panic!("expected a save, got {other:?}"),
    }
}

#[test]
fn the_folder_setting_is_used_only_when_absolute() {
    let default = Path::new("/home/me/Downloads");
    let mut flecha = Settings::default();
    assert_eq!(
        DownloadSettings::from_settings(&flecha, default),
        settings(false)
    );
    flecha.downloads_folder = Some("relative/folder".into());
    assert_eq!(
        DownloadSettings::from_settings(&flecha, default).folder,
        default
    );
    flecha.downloads_folder = Some(String::new());
    assert_eq!(
        DownloadSettings::from_settings(&flecha, default).folder,
        default
    );
    flecha.downloads_folder = Some("/data/incoming".into());
    flecha.ask_where_to_save = Some(true);
    assert_eq!(
        DownloadSettings::from_settings(&flecha, default),
        DownloadSettings {
            folder: PathBuf::from("/data/incoming"),
            ask_where_to_save: true,
        }
    );

    assert_eq!(
        set_downloads_folder(&mut flecha, Path::new("elsewhere")),
        Err(FolderError::NotAbsolute)
    );
    assert_eq!(
        set_downloads_folder(&mut flecha, Path::new("/media/usb")),
        Ok(())
    );
    assert_eq!(flecha.downloads_folder.as_deref(), Some("/media/usb"));
}

#[test]
fn downloads_save_in_the_folder_or_ask_first() {
    assert_eq!(
        saved(target(Case::default(), free)),
        Path::new("/home/me/Downloads/report.pdf")
    );
    let pictures = Path::new("/home/me/Pictures");
    assert_eq!(
        target(
            Case {
                ask: true,
                last_prompt_folder: Some(pictures),
                ..Case::default()
            },
            free
        ),
        TargetDecision::Ask {
            suggested: PathBuf::from("/home/me/Pictures/report.pdf"),
            reason: AskReason::Preference,
        }
    );
    // Without a prompt, the last dialog's folder doesn't matter.
    assert_eq!(
        saved(target(
            Case {
                last_prompt_folder: Some(pictures),
                ..Case::default()
            },
            free
        )),
        Path::new("/home/me/Downloads/report.pdf")
    );
    assert_eq!(
        target(
            Case {
                save_as: true,
                last_prompt_folder: Some(Path::new("relative")),
                ..Case::default()
            },
            free
        ),
        TargetDecision::Ask {
            suggested: PathBuf::from("/home/me/Downloads/report.pdf"),
            reason: AskReason::SaveAs,
        }
    );
}

#[test]
fn a_taken_name_gets_the_first_free_number_before_its_extension() {
    let taken: HashSet<PathBuf> = ["report.pdf", "report (1).pdf", "report (2).pdf"]
        .iter()
        .map(|name| Path::new(FOLDER).join(name))
        .collect();
    assert_eq!(
        saved(target(Case::default(), |p| taken.contains(p))),
        Path::new("/home/me/Downloads/report (3).pdf")
    );
    let archive = Path::new(FOLDER).join("backup.tar.gz");
    assert_eq!(
        saved(target(
            Case {
                name: "backup.tar.gz",
                ..Case::default()
            },
            |p| p == archive
        )),
        Path::new("/home/me/Downloads/backup (1).tar.gz")
    );
    let bare = Path::new(FOLDER).join("README");
    assert_eq!(
        saved(target(
            Case {
                name: "README",
                ..Case::default()
            },
            |p| p == bare
        )),
        Path::new("/home/me/Downloads/README (1)")
    );
    // A prompt suggests the free name too.
    assert_eq!(
        target(
            Case {
                ask: true,
                ..Case::default()
            },
            |p| taken.contains(p)
        ),
        TargetDecision::Ask {
            suggested: PathBuf::from("/home/me/Downloads/report (3).pdf"),
            reason: AskReason::Preference,
        }
    );
}

#[test]
fn after_a_hundred_numbers_comes_the_start_time_then_a_prompt() {
    let numbered = |p: &Path| {
        let name = p.file_name().unwrap().to_str().unwrap();
        name == "report.pdf" || (1..=MAX_UNIQUE_FILES).any(|n| name == format!("report ({n}).pdf"))
    };
    assert_eq!(
        saved(target(Case::default(), numbered)),
        Path::new("/home/me/Downloads/report - 2026-11-23T101530.123.pdf")
    );
    assert_eq!(
        target(Case::default(), |_| true),
        TargetDecision::Ask {
            suggested: PathBuf::from("/home/me/Downloads/report.pdf"),
            reason: AskReason::Conflict,
        }
    );
}

#[test]
fn downloads_in_progress_reserve_their_paths() {
    let mut list = DownloadList::new();
    let first = Path::new(FOLDER).join("report.pdf");
    list.begin(NewDownload {
        cef_id: 1,
        url: "https://example.com/report.pdf".into(),
        target: first.clone(),
        started_at: 1,
        warning: None,
        prompted: false,
    })
    .unwrap();
    assert_eq!(
        saved(target(Case::default(), |p| list.reserves(p, Platform::Linux))),
        Path::new("/home/me/Downloads/report (1).pdf")
    );
    // Chromium revokes the reservation once the download stops being in progress.
    list.update(
        1,
        Snapshot {
            state: SnapshotState::Complete,
            received: 1,
            total: 1,
            full_path: &first,
        },
    )
    .unwrap();
    assert_eq!(
        saved(target(Case::default(), |p| list.reserves(p, Platform::Linux))),
        first
    );
}

#[test]
fn long_names_shrink_before_the_extension_to_leave_room_for_crdownload() {
    let long = format!("{}.pdf", "a".repeat(300));
    let path = saved(target(
        Case {
            name: &long,
            ..Case::default()
        },
        free,
    ));
    let name = path.file_name().unwrap().to_str().unwrap();
    // 255 minus ".crdownload" (11).
    assert_eq!(name.len(), EXT4_NAME_MAX - ".crdownload".len());
    assert!(name.ends_with("a.pdf"));
    assert_eq!(path.parent(), Some(Path::new(FOLDER)));

    // UTF-8 is cut on a character boundary.
    let wide = format!("{}.txt", "é".repeat(200));
    let path = saved(target(
        Case {
            name: &wide,
            max: Some(EXT4_NAME_MAX + 1),
            ..Case::default()
        },
        free,
    ));
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(name.len() <= EXT4_NAME_MAX + 1 - ".crdownload".len());
    assert_eq!(name, format!("{}.txt", "é".repeat(120)));

    // Windows counts UTF-16 units and leaves room for ":Zone.Identifier" (16).
    let emoji = format!("{}.txt", "😀".repeat(200));
    let path = saved(target(
        Case {
            name: &emoji,
            max: Some(NTFS_NAME_MAX),
            platform: Platform::Windows,
            ..Case::default()
        },
        free,
    ));
    let name = path.file_name().unwrap().to_str().unwrap();
    assert!(name.encode_utf16().count() <= NTFS_NAME_MAX - ":Zone.Identifier".len());
    assert_eq!(name, format!("{}.txt", "😀".repeat(117)));

    // An unknown limit leaves the name alone.
    let path = saved(target(
        Case {
            name: &long,
            max: None,
            ..Case::default()
        },
        free,
    ));
    assert_eq!(path.file_name().unwrap().to_str().unwrap(), long);
}

#[test]
fn a_shortened_name_that_collides_is_numbered_within_the_limit() {
    let long = format!("{}.pdf", "a".repeat(300));
    let first = Path::new(FOLDER).join(format!("{}.pdf", "a".repeat(240)));
    let path = saved(target(
        Case {
            name: &long,
            ..Case::default()
        },
        |p| p == first,
    ));
    let name = path.file_name().unwrap().to_str().unwrap();
    assert_eq!(name, format!("{} (1).pdf", "a".repeat(236)));
    assert_eq!(name.len(), EXT4_NAME_MAX - ".crdownload".len());
}

#[test]
fn a_name_that_cannot_shrink_enough_asks_instead() {
    // The extension alone leaves fewer than five characters for the rest.
    let long_extension = format!("a.{}", "x".repeat(250));
    assert_eq!(
        target(
            Case {
                name: &long_extension,
                ..Case::default()
            },
            free
        ),
        TargetDecision::Ask {
            suggested: Path::new(FOLDER).join(&long_extension),
            reason: AskReason::NameTooLong,
        }
    );
    // A limit no bigger than ".crdownload" can't hold any name.
    assert!(matches!(
        target(
            Case {
                max: Some(11),
                ..Case::default()
            },
            free
        ),
        TargetDecision::Ask {
            reason: AskReason::NameTooLong,
            ..
        }
    ));
}

#[test]
fn every_hostile_name_lands_directly_in_the_folder() {
    let hostile = [
        "../../etc/passwd",
        "/etc/passwd",
        "..",
        ".",
        "",
        "C:\\Windows\\System32\\evil.dll",
        "\\\\server\\share\\x",
        "a/../../b",
        "evil\0name",
        "invoice\u{202E}fdp.exe",
        "CON",
    ];
    for platform in [Platform::Linux, Platform::Mac, Platform::Windows] {
        for name in hostile {
            for ask in [false, true] {
                let decision = target(
                    Case {
                        name,
                        ask,
                        platform,
                        ..Case::default()
                    },
                    free,
                );
                let path = decision.path();
                assert_eq!(
                    path.parent(),
                    Some(Path::new(FOLDER)),
                    "{name:?} {platform:?}"
                );
                assert_eq!(
                    path.components().count(),
                    Path::new(FOLDER).components().count() + 1,
                    "{name:?} {platform:?}"
                );
            }
        }
    }
}

#[cfg(unix)]
#[test]
fn the_file_system_limit_reads_from_pathconf() {
    let limit = flecha_downloads::name_max(&std::env::temp_dir()).expect("NAME_MAX");
    assert!(limit > ".crdownload".len());
    assert_eq!(
        flecha_downloads::name_max(Path::new("/no/such/folder/at/all")),
        None
    );
}

/// Where `decide` would save a link to `url` while `list` holds its downloads.
fn decided_target(url: &str, list: &DownloadList, platform: Platform) -> TargetDecision {
    let download_url = Url::parse(url).unwrap();
    decide(
        DownloadRequest {
            names: NameSources {
                url,
                content_disposition: None,
                suggested_name: None,
            },
            settings: &settings(false),
            save_as: false,
            max_name_length: Some(EXT4_NAME_MAX),
            started: started(),
            danger: DangerSignals {
                has_user_gesture: true,
                from_address_bar: false,
                visited_referrer_before: false,
            },
            insecure: InsecureSignals {
                download_url: &download_url,
                redirects: &[],
                initiator: None,
                trigger: Trigger::Navigation {
                    reload: false,
                    typed: false,
                    from_address_bar: false,
                    forward_back: false,
                    auto_toplevel: false,
                    auto_bookmark: false,
                    from_api: false,
                },
                https_first_mode: false,
            },
            platform,
        },
        list,
        |_: &Path| false,
    )
    .target
}

fn in_progress(list: &mut DownloadList, cef_id: u32, name: &str, prompted: bool) {
    list.begin(NewDownload {
        cef_id,
        url: format!("https://example.com/{cef_id}"),
        target: Path::new(FOLDER).join(name),
        started_at: 1,
        warning: None,
        prompted,
    })
    .unwrap();
}

fn saves_to(name: &str) -> TargetDecision {
    TargetDecision::Save(Path::new(FOLDER).join(name))
}

#[test]
fn reservations_ignore_case_as_chromium_does_on_each_platform() {
    let mut list = DownloadList::new();
    in_progress(&mut list, 1, "Report.pdf", false);
    in_progress(&mut list, 2, "\u{c4}rger.pdf", false);
    for platform in [Platform::Linux, Platform::Mac, Platform::Windows] {
        assert_eq!(
            decided_target("https://example.com/report.pdf", &list, platform),
            saves_to("report (1).pdf"),
            "{platform:?}"
        );
    }
    // macOS and Windows fold non-ASCII letters too; Linux's `strncasecmp` folds ASCII only.
    let lower_umlaut = "https://example.com/%C3%A4rger.pdf";
    for platform in [Platform::Mac, Platform::Windows] {
        assert_eq!(
            decided_target(lower_umlaut, &list, platform),
            saves_to("\u{e4}rger (1).pdf"),
            "{platform:?}"
        );
    }
    assert_eq!(
        decided_target(lower_umlaut, &list, Platform::Linux),
        saves_to("\u{e4}rger.pdf")
    );
    // macOS compares the decomposed form: a decomposed "A" plus diaeresis is the same name.
    assert_eq!(
        decided_target("https://example.com/A%CC%88rger.pdf", &list, Platform::Mac),
        saves_to("A\u{308}rger (1).pdf")
    );
}

#[test]
fn a_path_picked_in_the_save_dialog_is_reserved_once_reported() {
    let mut list = DownloadList::new();
    in_progress(&mut list, 4, "a.pdf", true);
    list.update(
        4,
        Snapshot {
            state: SnapshotState::InProgress,
            received: 1,
            total: 9,
            full_path: &Path::new(FOLDER).join("x.pdf.crdownload"),
        },
    )
    .unwrap();
    assert_eq!(
        decided_target("https://example.com/x.pdf", &list, Platform::Linux),
        saves_to("x (1).pdf")
    );
    // The suggestion it replaced is free again.
    assert_eq!(
        decided_target("https://example.com/a.pdf", &list, Platform::Linux),
        saves_to("a.pdf")
    );
}
