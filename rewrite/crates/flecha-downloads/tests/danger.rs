//! Held downloads: Chromium's file-type policy (`download_file_types.asciipb`, version 69),
//! `GetDangerLevel`, and `GetInsecureDownloadStatusForDownload`.

use std::path::{Path, PathBuf};

use chrono::{NaiveDate, NaiveDateTime};
use flecha_downloads::{
    DangerLevel, DangerSignals, DownloadList, DownloadRequest, DownloadSettings, FileTypePolicies,
    InsecureSignals, InsecureVerdict, NameSources, Platform, Trigger, Verdict, Warning, decide,
    file_danger, insecure_verdict,
};
use url::Url;

const NO_SIGNALS: DangerSignals = DangerSignals {
    has_user_gesture: false,
    from_address_bar: false,
    visited_referrer_before: false,
};

const LINK: Trigger = Trigger::Navigation {
    reload: false,
    typed: false,
    from_address_bar: false,
    forward_back: false,
    auto_toplevel: false,
    auto_bookmark: false,
    from_api: false,
};

fn level(platform: Platform, name: &str) -> DangerLevel {
    FileTypePolicies::chromium(platform).danger_level(name)
}

#[test]
fn the_vendored_policy_is_chromium_158s_and_reads_completely() {
    for platform in [Platform::Linux, Platform::Mac, Platform::Windows] {
        let policies = FileTypePolicies::chromium(platform);
        assert_eq!(policies.version_id(), 69);
        // `grep -c 'extension:'` on the file: one entry per extension, no duplicates.
        assert_eq!(policies.extension_count(), 412);
    }
}

#[test]
fn the_policy_parser_rejects_what_it_does_not_know() {
    let base = "version_id: 1\ndefault_file_type {\n  platform_settings {\n    danger_level: NOT_DANGEROUS\n  }\n}\n";
    assert!(FileTypePolicies::parse(base, Platform::Linux).is_ok());
    let unknown_field = format!("{base}surprise: 1\n");
    assert!(FileTypePolicies::parse(&unknown_field, Platform::Linux).is_err());
    let unknown_level = base.replace("NOT_DANGEROUS", "SOMEWHAT_DANGEROUS");
    assert!(FileTypePolicies::parse(&unknown_level, Platform::Linux).is_err());
    let unclosed = base.trim_end().trim_end_matches('}');
    assert!(FileTypePolicies::parse(unclosed, Platform::Linux).is_err());
    let no_default = "version_id: 1\n";
    assert!(FileTypePolicies::parse(no_default, Platform::Linux).is_err());
}

#[test]
fn each_platform_gets_its_own_setting_then_the_shared_one_then_the_default() {
    use DangerLevel::*;
    // "deb" has only a Linux setting.
    assert_eq!(level(Platform::Linux, "setup.deb"), AllowOnUserGesture);
    assert_eq!(level(Platform::Windows, "setup.deb"), NotDangerous);
    // "exe" has only a Windows setting.
    assert_eq!(level(Platform::Windows, "setup.exe"), AllowOnUserGesture);
    assert_eq!(level(Platform::Linux, "setup.exe"), NotDangerous);
    // "sh" is ALLOW_ON_USER_GESTURE everywhere except Windows, which is NOT_DANGEROUS.
    assert_eq!(level(Platform::Linux, "run.sh"), AllowOnUserGesture);
    assert_eq!(level(Platform::Mac, "run.sh"), AllowOnUserGesture);
    assert_eq!(level(Platform::Windows, "run.sh"), NotDangerous);
    // DANGEROUS on Windows.
    assert_eq!(level(Platform::Windows, "x.local"), Dangerous);
    assert_eq!(level(Platform::Windows, "x.dll"), Dangerous);
    assert_eq!(level(Platform::Mac, "x.dmg"), AllowOnUserGesture);
    assert_eq!(level(Platform::Linux, "report.pdf"), NotDangerous);
    assert_eq!(level(Platform::Linux, "photo.jpg"), NotDangerous);
}

#[test]
fn extensions_compare_like_chromium_canonicalizes_them() {
    use DangerLevel::*;
    assert_eq!(level(Platform::Linux, "SETUP.DEB"), AllowOnUserGesture);
    assert_eq!(level(Platform::Linux, "setup.deb. . "), AllowOnUserGesture);
    assert_eq!(level(Platform::Linux, "setup.deb.txt"), NotDangerous);
    assert_eq!(level(Platform::Linux, "deb"), NotDangerous);
    assert_eq!(level(Platform::Linux, ".deb"), AllowOnUserGesture);
    assert_eq!(level(Platform::Linux, "setup.débé"), NotDangerous);
}

#[test]
fn allow_on_user_gesture_is_waived_for_typed_or_familiar_downloads() {
    let linux = FileTypePolicies::chromium(Platform::Linux);
    let deb = |signals| file_danger(linux, "setup.deb", signals);
    assert_eq!(deb(NO_SIGNALS), DangerLevel::AllowOnUserGesture);
    let gesture = DangerSignals {
        has_user_gesture: true,
        ..NO_SIGNALS
    };
    assert_eq!(deb(gesture), DangerLevel::AllowOnUserGesture);
    let visited = DangerSignals {
        visited_referrer_before: true,
        ..NO_SIGNALS
    };
    assert_eq!(deb(visited), DangerLevel::AllowOnUserGesture);
    let familiar = DangerSignals {
        has_user_gesture: true,
        visited_referrer_before: true,
        ..NO_SIGNALS
    };
    assert_eq!(deb(familiar), DangerLevel::NotDangerous);
    let typed = DangerSignals {
        from_address_bar: true,
        ..NO_SIGNALS
    };
    assert_eq!(deb(typed), DangerLevel::NotDangerous);

    let windows = FileTypePolicies::chromium(Platform::Windows);
    let everything = DangerSignals {
        has_user_gesture: true,
        from_address_bar: true,
        visited_referrer_before: true,
    };
    assert_eq!(
        file_danger(windows, "x.local", everything),
        DangerLevel::Dangerous
    );
}

fn url(text: &str) -> Url {
    Url::parse(text).unwrap()
}

fn verdict(
    name: &str,
    chain: &[&str],
    initiator: Option<&str>,
    trigger: Trigger,
) -> InsecureVerdict {
    let chain: Vec<Url> = chain.iter().map(|u| url(u)).collect();
    let initiator = initiator.map(url);
    insecure_verdict(
        name,
        InsecureSignals {
            url_chain: &chain,
            initiator: initiator.as_ref(),
            trigger,
            https_first_mode: false,
        },
    )
}

#[test]
fn plain_http_downloads_are_held_unless_likely_safe() {
    use InsecureVerdict::*;
    let http = "http://downloads.example/held.bin";
    assert_eq!(
        verdict("held.bin", &[http], Some("http://example.com/"), LINK),
        Hold
    );
    assert_eq!(verdict("held.bin", &[http], None, LINK), Hold);
    assert_eq!(verdict("notes.TXT", &[http], None, LINK), Safe);
    let https_first = insecure_verdict(
        "notes.txt",
        InsecureSignals {
            url_chain: &[url(http)],
            initiator: None,
            trigger: LINK,
            https_first_mode: true,
        },
    );
    assert_eq!(https_first, Hold);
    // An insecure page that sends you to a secure file still can't be trusted.
    assert_eq!(
        verdict(
            "a.bin",
            &["https://cdn.example/a.bin"],
            Some("http://example.com/"),
            LINK
        ),
        Hold
    );
    // A redirect through plain HTTP taints the delivery.
    assert_eq!(
        verdict(
            "a.bin",
            &["http://example.com/go", "https://cdn.example/a.bin"],
            None,
            LINK
        ),
        Hold
    );
}

#[test]
fn secure_local_and_exempt_downloads_are_not_held() {
    use InsecureVerdict::*;
    let secure = "https://cdn.example/a.bin";
    assert_eq!(
        verdict("a.bin", &[secure], Some("https://example.com/"), LINK),
        Safe
    );
    assert_eq!(verdict("a.bin", &[secure], Some("null:opaque"), LINK), Safe);
    assert_eq!(
        verdict("a.bin", &["http://localhost:8080/a.bin"], None, LINK),
        Safe
    );
    assert_eq!(
        verdict("a.bin", &["http://127.0.0.2/a.bin"], None, LINK),
        Safe
    );
    assert_eq!(verdict("a.bin", &["http://[::1]/a.bin"], None, LINK), Safe);
    assert_eq!(
        verdict("a.bin", &["http://dev.localhost/a.bin"], None, LINK),
        Safe
    );
    assert_eq!(
        verdict(
            "a.bin",
            &["blob:https://example.com/0f1e"],
            Some("https://example.com/"),
            LINK
        ),
        Safe
    );
    assert_eq!(
        verdict("a.bin", &["data:application/octet-stream,x"], None, LINK),
        Safe
    );
    assert_eq!(
        verdict("a.bin", &["http://example.com/a.bin"], None, Trigger::Retry),
        Safe
    );
    assert_eq!(
        verdict(
            "a.bin",
            &["http://example.com/a.bin"],
            None,
            Trigger::Internal
        ),
        Safe
    );
    let reload = Trigger::Navigation {
        reload: true,
        typed: false,
        from_address_bar: false,
        forward_back: false,
        auto_toplevel: false,
        auto_bookmark: false,
        from_api: false,
    };
    assert_eq!(
        verdict("a.bin", &["http://example.com/a.bin"], None, reload),
        Safe
    );
}

#[test]
fn mixed_content_is_blocked_silently_unless_the_person_asked() {
    use InsecureVerdict::*;
    let chain = ["http://cdn.example/a.bin"];
    let page = Some("https://example.com/");
    assert_eq!(verdict("a.bin", &chain, page, LINK), SilentBlock);
    // Even likely-safe types: the safe list only applies to non-mixed downloads.
    assert_eq!(verdict("a.txt", &chain, page, LINK), SilentBlock);
    assert_eq!(verdict("a.bin", &chain, page, Trigger::ContextMenu), Hold);
    assert_eq!(
        verdict("a.bin", &chain, page, Trigger::WebContentsApi),
        Hold
    );
    // Typed into the address bar: not mixed content, but still insecure.
    let typed = Trigger::Navigation {
        reload: false,
        typed: true,
        from_address_bar: true,
        forward_back: false,
        auto_toplevel: false,
        auto_bookmark: false,
        from_api: false,
    };
    assert_eq!(verdict("a.bin", &chain, page, typed), Hold);
}

fn started() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, 11, 23)
        .unwrap()
        .and_hms_milli_opt(10, 15, 30, 123)
        .unwrap()
}

fn decided(
    url_text: &str,
    initiator: Option<&str>,
    ask: bool,
    danger: DangerSignals,
) -> (Verdict, PathBuf) {
    let settings = DownloadSettings {
        folder: PathBuf::from("/home/me/Downloads"),
        ask_where_to_save: ask,
    };
    let chain = [url(url_text)];
    let initiator = initiator.map(url);
    let decision = decide(
        DownloadRequest {
            names: NameSources {
                url: url_text,
                content_disposition: None,
                suggested_name: None,
            },
            settings: &settings,
            save_as: false,
            max_name_length: Some(255),
            started: started(),
            danger,
            insecure: InsecureSignals {
                url_chain: &chain,
                initiator: initiator.as_ref(),
                trigger: LINK,
                https_first_mode: false,
            },
            platform: Platform::Linux,
        },
        &DownloadList::new(),
        |_: &Path| false,
    );
    (decision.verdict, decision.target.path().to_path_buf())
}

#[test]
fn decide_holds_insecure_before_dangerous_and_trusts_a_chosen_path() {
    let page = Some("https://example.com/");
    let (verdict, path) = decided("https://example.com/setup.deb", page, false, NO_SIGNALS);
    assert_eq!(verdict, Verdict::Hold(Warning::Dangerous));
    assert_eq!(path, Path::new("/home/me/Downloads/setup.deb"));
    // A path picked in the save dialog approves the file type.
    let (verdict, _) = decided("https://example.com/setup.deb", page, true, NO_SIGNALS);
    assert_eq!(verdict, Verdict::Proceed);
    let familiar = DangerSignals {
        has_user_gesture: true,
        visited_referrer_before: true,
        from_address_bar: false,
    };
    let (verdict, _) = decided("https://example.com/setup.deb", page, false, familiar);
    assert_eq!(verdict, Verdict::Proceed);
    // Insecure is reported first, as patch 0058's WarningKind does.
    let (verdict, _) = decided("http://example.com/setup.deb", None, false, NO_SIGNALS);
    assert_eq!(verdict, Verdict::Hold(Warning::Insecure));
    // Chromium checks insecure delivery even for a dialog-chosen path.
    let (verdict, _) = decided("http://example.com/setup.deb", None, true, NO_SIGNALS);
    assert_eq!(verdict, Verdict::Hold(Warning::Insecure));
    let (verdict, _) = decided("http://cdn.example/a.bin", page, false, NO_SIGNALS);
    assert_eq!(verdict, Verdict::Block);
    let (verdict, _) = decided("https://example.com/report.pdf", page, false, NO_SIGNALS);
    assert_eq!(verdict, Verdict::Proceed);
}
