//! The rewrite's Boosts file, its damaged-file fallback, and the shell's stored shape.

use flecha_boosts::snapshot::{
    self, BackupWrite, DecodeError, Recovery, SCHEMA_VERSION, Slot, SnapshotRepairs,
};
use flecha_boosts::{
    Boosts, CompiledBoost, Frame, SHELL_BUILDS_KEY, SHELL_FILES_KEY, applies, boost_matches,
};
use serde_json::{Value, json};

fn sample() -> Boosts {
    let mut boosts = Boosts::new();
    boosts.save("zeta.example.css", "p {}", None, true).unwrap();
    let build = CompiledBoost {
        code: Some("run()".into()),
        error: None,
    };
    boosts.save("all.ts", "run()", Some(build), true).unwrap();
    boosts.save("alpha.example.ts", "", None, false).unwrap();
    boosts
}

fn text(boosts: &Boosts) -> Value {
    Value::String(snapshot::encode(boosts))
}

#[test]
fn a_snapshot_round_trips_in_apply_order() {
    let boosts = sample();
    let decoded = snapshot::decode(&snapshot::encode(&boosts)).unwrap();
    assert_eq!(decoded.boosts, boosts);
    assert_eq!(decoded.repairs, SnapshotRepairs::default());
    let names: Vec<_> = decoded
        .boosts
        .iter()
        .map(|(name, _, _)| name.to_string())
        .collect();
    assert_eq!(names, ["zeta.example.css", "all.ts", "alpha.example.ts"]);
}

#[test]
fn decoding_drops_unusable_entries_and_builds() {
    let json = json!({
        "version": SCHEMA_VERSION,
        "boosts": [
            { "name": "ok.example.ts", "code": "a", "compiled": { "code": 1 } },
            { "name": "ok.example.ts", "code": "duplicate" },
            { "name": "no-extension", "code": "x" },
            { "name": "x.example.css", "code": 3 },
            "not an object",
            { "name": "kept.example.css", "code": "b", "future": true },
        ],
        "future": [],
    });
    let decoded = snapshot::decode(&json.to_string()).unwrap();
    assert_eq!(
        decoded.repairs,
        SnapshotRepairs {
            skipped_boosts: 4,
            dropped_builds: 1
        }
    );
    let names: Vec<_> = decoded
        .boosts
        .iter()
        .map(|(n, c, b)| (n.to_string(), c.to_owned(), b.cloned()))
        .collect();
    assert_eq!(
        names,
        [
            ("ok.example.ts".into(), "a".into(), None),
            ("kept.example.css".into(), "b".into(), None)
        ]
    );
}

#[test]
fn versions_and_shapes() {
    for (json, newer) in [
        (json!({ "version": 2, "boosts": [] }), true),
        (json!({ "version": 1.5, "boosts": [] }), true),
        (json!({ "version": "2", "boosts": [] }), true),
        (json!({ "version": 0, "boosts": [] }), false),
        (json!({ "version": "v1", "boosts": [] }), false),
        (json!({ "boosts": [] }), false),
        (json!({ "version": 1, "boosts": {} }), false),
    ] {
        match snapshot::decode(&json.to_string()) {
            Err(DecodeError::UnsupportedVersion(_)) => assert!(newer, "{json}"),
            Err(DecodeError::InvalidShape) => assert!(!newer, "{json}"),
            other => panic!("{json}: {other:?}"),
        }
    }
    // The version is read as flecha-core reads its own: a numeric string counts.
    assert!(snapshot::decode(&json!({ "version": "1", "boosts": [] }).to_string()).is_ok());
    for json in ["{", "[]", "\"text\""] {
        assert!(
            matches!(snapshot::decode(json), Err(DecodeError::Json(_))),
            "{json}"
        );
    }
}

/// The version is read before anything else, so a newer file is refused, never set aside as
/// damaged, whatever its other fields hold.
#[test]
fn a_newer_snapshot_is_refused_whatever_it_holds() {
    let deep = format!("{}1{}", "[".repeat(200), "]".repeat(200));
    let newer = [
        r#"{"version":2,"boosts":[],"x":1e400}"#.to_owned(),
        r#"{"version":2,"boosts":[],"x":"\ud800"}"#.to_owned(),
        format!(r#"{{"version":2,"boosts":[],"x":{deep}}}"#),
        r#"{"version":"2","boosts":[]}"#.to_owned(),
    ];
    for body in newer {
        let value = Value::String(body.clone());
        assert!(
            matches!(
                snapshot::load(Some(&value), Some(&text(&sample()))),
                Err(DecodeError::UnsupportedVersion(_))
            ),
            "current {body}"
        );
        let mut loaded = snapshot::load(Some(&json!("{")), Some(&value)).unwrap();
        let mut writes = Vec::new();
        let ready = loaded
            .apply_recovery(|write| {
                writes.push((write.slot(), write.value().clone()));
                Ok::<_, ()>(())
            })
            .unwrap();
        assert_eq!(
            ready.recovered,
            Some(Recovery::UnsupportedBackup),
            "last-good {body}"
        );
        assert_eq!(writes, [(Slot::Damaged, json!("{"))], "last-good {body}");
    }
    // The same text at the current version is damaged: it can't be read.
    let current = r#"{"version":1,"boosts":[],"x":1e400}"#;
    assert!(matches!(
        snapshot::decode(current),
        Err(DecodeError::Json(_))
    ));
}

#[test]
fn a_clean_start_promotes_last_good_after_start() {
    let boosts = sample();
    let mut loaded = snapshot::load(Some(&text(&boosts)), None).unwrap();
    let ready = loaded.ready().expect("no recovery pending");
    assert_eq!(ready.boosts, boosts);
    assert_eq!(ready.recovered, None);
    let promoted = ready.started().expect("a last-good write");
    assert_eq!(
        (promoted.slot(), promoted.value()),
        (Slot::LastGood, &text(&boosts))
    );
    assert_eq!(ready.started(), None);
    assert_eq!(ready.save_current(&boosts).slot(), Slot::Current);

    let mut unchanged = snapshot::load(Some(&text(&boosts)), Some(&text(&boosts))).unwrap();
    assert_eq!(unchanged.ready().unwrap().started(), None);
}

#[test]
fn a_damaged_snapshot_is_set_aside_before_last_good_replaces_it() {
    let good = sample();
    let damaged = Value::String("{ not json".into());
    let mut loaded = snapshot::load(Some(&damaged), Some(&text(&good))).unwrap();
    assert!(loaded.ready().is_none(), "the writer waits for recovery");

    let mut writes: Vec<BackupWrite> = Vec::new();
    let mut fail_once = true;
    let failed = loaded.apply_recovery(|write| {
        writes.push(write.clone());
        if write.slot() == Slot::Current && std::mem::take(&mut fail_once) {
            return Err("disk full");
        }
        Ok(())
    });
    assert_eq!(failed.err(), Some("disk full"));
    assert!(loaded.ready().is_none());
    let ready = loaded
        .apply_recovery(|write| {
            writes.push(write.clone());
            Ok::<_, ()>(())
        })
        .unwrap();
    assert_eq!(ready.boosts, good);
    assert_eq!(ready.recovered, Some(Recovery::LastGood));
    let slots: Vec<_> = writes
        .iter()
        .map(|w| (w.slot(), w.value().clone()))
        .collect();
    assert_eq!(
        slots,
        [
            (Slot::Damaged, damaged.clone()),
            (Slot::Current, text(&good)),
            (Slot::Current, text(&good)),
        ]
    );
    assert!(loaded.ready().is_some());
}

#[test]
fn damaged_without_a_usable_backup() {
    let not_a_string = json!({ "version": 1, "boosts": [] });
    let mut fresh = snapshot::load(Some(&not_a_string), Some(&json!("{"))).unwrap();
    let ready = fresh.apply_recovery(|_| Ok::<_, ()>(())).unwrap();
    assert_eq!(ready.recovered, Some(Recovery::Fresh));
    assert!(ready.boosts.is_empty());

    let newer = Value::String(json!({ "version": 2, "boosts": [] }).to_string());
    let mut unsupported = snapshot::load(Some(&json!("{")), Some(&newer)).unwrap();
    let ready = unsupported.apply_recovery(|_| Ok::<_, ()>(())).unwrap();
    assert_eq!(ready.recovered, Some(Recovery::UnsupportedBackup));
}

#[test]
fn a_newer_snapshot_is_never_overwritten() {
    let newer = Value::String(json!({ "version": 2, "boosts": [] }).to_string());
    assert!(matches!(
        snapshot::load(Some(&newer), None),
        Err(DecodeError::UnsupportedVersion(_))
    ));
    let mut loaded = snapshot::load(Some(&text(&sample())), Some(&newer)).unwrap();
    assert_eq!(
        loaded.ready().unwrap().started(),
        None,
        "keep the newer last-good copy"
    );
}

#[test]
fn the_shells_stored_shape() {
    let files = json!({
        "b.example.css": "b {}",
        "a.example.ts": "a()",
        "notes.txt": "x",
        "c.example.ts": 3,
    });
    let builds = json!({
        "a.example.ts": { "code": "wrapped()" },
        "gone.example.ts": { "code": "orphan" },
        "b.example.css": { "code": 1 },
    });
    let import = Boosts::from_shell(Some(&files), Some(&builds));
    assert_eq!(import.skipped, ["notes.txt", "c.example.ts"]);
    assert_eq!(import.dropped_builds, ["b.example.css"]);
    assert!(import.unreadable.is_empty());
    let names: Vec<_> = import
        .boosts
        .iter()
        .map(|(n, _, _)| n.to_string())
        .collect();
    assert_eq!(names, ["b.example.css", "a.example.ts"]);
    assert_eq!(
        import
            .boosts
            .scripts_for(Frame::Main, "https://a.example/")
            .len(),
        1
    );

    let empty = Boosts::from_shell(None, Some(&Value::Null));
    assert!(empty.boosts.is_empty() && empty.unreadable.is_empty());
}

/// The shell read any non-object value as having no Boosts, or no builds, and kept going.
#[test]
fn malformed_shell_values_import_what_the_shell_used() {
    let files = json!({ "a.example.css": "a {}", "b.example.ts": "b()" });
    for builds in [json!([]), json!("x"), json!(5), json!(true)] {
        let import = Boosts::from_shell(Some(&files), Some(&builds));
        assert_eq!(import.unreadable, [SHELL_BUILDS_KEY], "{builds}");
        assert_eq!(import.boosts.list().len(), 2, "{builds}");
        assert_eq!(
            import
                .boosts
                .styles_for(Frame::Main, "https://a.example/")
                .len(),
            1
        );
        assert!(
            import
                .boosts
                .scripts_for(Frame::Main, "https://b.example/")
                .is_empty()
        );
    }
    for files in [json!(["a.example.css"]), json!("a.example.css"), json!(5)] {
        let import = Boosts::from_shell(Some(&files), None);
        assert_eq!(import.unreadable, [SHELL_FILES_KEY], "{files}");
        assert!(import.boosts.is_empty(), "{files}");
    }
}

/// A deliberate difference from the shell. As a component extension, the shell styled and ran
/// Boosts on `chrome://` and extension pages, and reloaded `file:` pages. The rewrite limits
/// Boosts to http and https; `tests/parity.rs` records what the shell did.
#[test]
fn the_rewrite_limits_boosts_to_web_pages() {
    for (url, host) in [
        ("chrome://settings/", "settings"),
        ("flecha://boosts", "boosts"),
        ("file://server/share", "server"),
        (
            "chrome-extension://abcdefghijklmnop/boosts.html",
            "abcdefghijklmnop",
        ),
        ("ws://github.com/", "github.com"),
        ("view-source:https://github.com/", "github.com"),
    ] {
        assert_eq!(
            boost_matches(url, host),
            url != "view-source:https://github.com/",
            "{url}"
        );
        assert!(!applies(url, host), "{url}");
    }
    assert!(applies("https://github.com/", "github.com"));
    assert!(applies("http://gist.github.com/", "all"));

    let mut boosts = Boosts::new();
    boosts.save("settings.css", "body {}", None, true).unwrap();
    assert!(
        boosts
            .styles_for(Frame::Main, "chrome://settings/")
            .is_empty()
    );
    assert!(
        boosts
            .styles_for(Frame::Sub, "https://settings/")
            .is_empty()
    );
    assert_eq!(boosts.styles_for(Frame::Main, "https://settings/").len(), 1);
}

/// Chromium registers user scripts all or nothing (`user_scripts_api.cc`), so one host it
/// rejects leaves every script to the shell's execute path.
#[test]
fn one_rejected_pattern_disables_every_registered_script() {
    let build = |code: &str| {
        Some(CompiledBoost {
            code: Some(code.into()),
            error: None,
        })
    };
    let files = json!({ "github.com.ts": "a", "q b.ts": "b" });
    let builds = json!({ "github.com.ts": { "code": "a()" }, "q b.ts": { "code": "b()" } });
    let mut boosts = Boosts::from_shell(Some(&files), Some(&builds)).boosts;
    assert!(
        boosts
            .scripts_for(Frame::Main, "https://github.com./")
            .is_empty()
    );
    assert_eq!(
        boosts.scripts_for(Frame::Main, "https://github.com/").len(),
        1
    );
    boosts.remove("q b.ts");
    assert_eq!(
        boosts
            .scripts_for(Frame::Main, "https://github.com./")
            .len(),
        1
    );
    boosts.save("all.ts", "", build(""), true).unwrap();
    assert_eq!(
        boosts
            .scripts_for(Frame::Main, "https://github.com./")
            .len(),
        1
    );
    boosts.save("all.ts", "", build("every()"), true).unwrap();
    assert_eq!(
        boosts
            .scripts_for(Frame::Main, "https://github.com./")
            .len(),
        2
    );
}
