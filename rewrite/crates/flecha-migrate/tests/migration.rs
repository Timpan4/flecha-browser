#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use flecha_core::{SpaceId, TabId, Theme, WindowId, storage};
use flecha_migrate::{
    LEGACY_BOOSTS_FILE, LEGACY_COMPILED_BOOSTS_FILE, MigrationReport, ProfileInUse,
    SHELL_EXTENSION_ID, StateSource, migrate_profile,
};
use flecha_services::StateStore;
use rusty_leveldb::{CompressorId, DB, Options, compressor::SnappyCompressor};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

struct Fixture {
    _scratch: TempDir,
    old: PathBuf,
    leveldb: PathBuf,
    new: PathBuf,
}

impl Fixture {
    fn empty() -> Result<Self> {
        let scratch = tempfile::tempdir()?;
        let parent = scratch.path().canonicalize()?;
        let old = parent.join("chromium");
        let new = parent.join("rewrite");
        let leveldb = old
            .join("Default/Local Extension Settings")
            .join(SHELL_EXTENSION_ID);
        fs::create_dir_all(&leveldb)?;
        Ok(Self {
            _scratch: scratch,
            old,
            leveldb,
            new,
        })
    }

    fn write(entries: &[(&str, &[u8])]) -> Result<Self> {
        let fixture = Self::empty()?;
        let mut db = DB::open(&fixture.leveldb, Options::default())?;
        for (key, value) in entries {
            db.put(key.as_bytes(), value)?;
        }
        db.close()?;
        Ok(fixture)
    }

    fn cpp_leveldb(name: &str) -> Result<Self> {
        let fixture = Self::empty()?;
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/leveldb")
            .join(name);
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            fs::copy(entry.path(), fixture.leveldb.join(entry.file_name()))?;
        }
        Ok(fixture)
    }

    fn compact_with_snappy(&self) -> Result<()> {
        let mut db = DB::open(
            &self.leveldb,
            Options {
                compressor: SnappyCompressor::ID,
                ..Options::default()
            },
        )?;
        db.compact_range(b"", b"\xff")?;
        db.close()?;
        assert!(
            fs::read_dir(&self.leveldb)?
                .collect::<std::io::Result<Vec<_>>>()?
                .iter()
                .any(|entry| entry.path().extension() == Some(OsStr::new("ldb")))
        );
        Ok(())
    }

    fn migrate(&self) -> Result<MigrationReport> {
        let before = hash_tree(&self.old)?;
        let result = migrate_profile(&self.old, OsStr::new("Default"), &self.new);
        assert_eq!(hash_tree(&self.old)?, before);
        result
    }

    fn load(&self) -> Result<Option<flecha_core::BrowserState>> {
        assert!(self.new.is_dir());
        let (_, mut loaded) = StateStore::open(&self.new.join("session"))?;
        let ready = loaded.ready().expect("published state must load directly");
        assert_eq!(ready.recovered, None);
        Ok(ready.state.take())
    }
}

#[derive(Debug, Eq, PartialEq)]
enum EntryHash {
    Directory,
    File([u8; 32]),
    Symlink(PathBuf),
}

fn hash_tree(root: &Path) -> Result<BTreeMap<PathBuf, EntryHash>> {
    fn walk(root: &Path, path: &Path, hashes: &mut BTreeMap<PathBuf, EntryHash>) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let path = entry.path();
            let kind = entry.file_type()?;
            let hash = if kind.is_symlink() {
                EntryHash::Symlink(fs::read_link(&path)?)
            } else if kind.is_dir() {
                walk(root, &path, hashes)?;
                EntryHash::Directory
            } else {
                assert!(kind.is_file());
                EntryHash::File(Sha256::digest(fs::read(&path)?).into())
            };
            hashes.insert(path.strip_prefix(root)?.to_owned(), hash);
        }
        Ok(())
    }
    let mut hashes = BTreeMap::new();
    walk(root, root, &mut hashes)?;
    Ok(hashes)
}

fn snapshot(version: Option<u64>, path: &str) -> Vec<u8> {
    let mut state = json!({
        "spaces": [{"id": "personal", "name": "Personal", "color": "#aabbcc", "pinned": []}],
        "tabs": {"tab": {"url": format!("https://example.test/{path}"), "title": "Tab"}},
        "theme": "dark",
        "adBlock": true,
        "adBlockAllowed": ["good.test", "[::1]"],
        "archive": [{"url": "https://example.test/archive", "title": "Archive", "closedAt": 1}],
    });
    if let Some(version) = version {
        state["version"] = version.into();
    }
    if version == Some(2) {
        state["windows"] = json!([{
            "id": "original", "activeSpaceId": "personal",
            "spaces": {"personal": {"today": ["tab"], "activeTabId": "tab"}},
        }]);
    } else {
        state["activeSpaceId"] = "personal".into();
        state["spaces"][0]["today"] = json!(["tab"]);
        state["spaces"][0]["activeTabId"] = "tab".into();
    }
    serde_json::to_vec(&state.to_string()).unwrap()
}

fn assert_session(state: &flecha_core::BrowserState, path: &str) {
    assert_eq!(state.spaces()[0].id, SpaceId::new("personal"));
    assert_eq!(state.windows().len(), 1);
    assert_eq!(
        state.tab(&TabId::new("tab")).unwrap().url,
        format!("https://example.test/{path}"),
    );
    assert_eq!(state.settings.theme, Theme::Dark);
    assert_eq!(state.settings.ad_block, Some(true));
    assert_eq!(
        state.settings.ad_block_allowed,
        Some(vec!["good.test".into()])
    );
}

#[test]
fn cli_reports_migrated_counts() -> Result<()> {
    let fixture = Fixture::write(&[
        ("state", &snapshot(Some(2), "current")),
        ("boosts", br#"{"site.ts":"export {};","all.css":"body {}"}"#),
    ])?;
    let output = Command::new(env!("CARGO_BIN_EXE_flecha-migrate"))
        .arg(&fixture.old)
        .arg("Default")
        .arg(&fixture.new)
        .output()?;
    let stderr = String::from_utf8(output.stderr)?;
    assert!(output.status.success(), "{stderr}");
    assert_eq!(
        String::from_utf8(output.stdout)?,
        "Spaces: 1\nWindows: 1\nOpen tabs: 1\nArchive entries: 1\nAd-block sites: 1\nBoosts: 2\n"
    );
    assert_session(&fixture.load()?.unwrap(), "current");
    Ok(())
}

#[test]
fn cli_returns_failure_for_a_locked_profile() -> Result<()> {
    let fixture = Fixture::empty()?;
    fs::write(fixture.old.join("SingletonLock"), b"")?;
    let output = Command::new(env!("CARGO_BIN_EXE_flecha-migrate"))
        .arg(&fixture.old)
        .arg("Default")
        .arg(&fixture.new)
        .output()?;
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8(output.stderr)?.contains("Chromium profile is in use"));
    assert!(!fixture.new.exists());
    Ok(())
}

#[test]
fn migrates_unversioned_v1_and_v2_profiles_read_only() -> Result<()> {
    for version in [None, Some(1), Some(2)] {
        let primary = snapshot(version, "current");
        let recovery = snapshot(version, "previous");
        let boosts = br#"{ "example.test.ts": "export {};", "all.css": "body {}" }"#;
        let compiled = br#"{"example.test.ts":{"code":"export {};"}}"#;
        let fixture = Fixture::write(&[
            ("state", &primary),
            ("state:last-good", &recovery),
            ("state:before-import", &recovery),
            ("state:damaged", b"null"),
            ("boosts", boosts),
            ("boostsCompiled", compiled),
            ("future:key", b"not required to parse"),
        ])?;
        if version == Some(2) {
            fixture.compact_with_snappy()?;
        }
        let report = fixture.migrate()?;
        assert_eq!(report.state_source, StateSource::Primary);
        assert_eq!(report.migrated_keys, 5);
        assert_eq!(report.invalid_keys, 0);
        assert_eq!(report.unknown_keys, 1);
        assert_eq!(report.damaged_backups, 1);
        assert_eq!((report.spaces, report.windows, report.open_tabs), (1, 1, 1));
        assert_eq!((report.archive_entries, report.ad_block_sites), (1, 1));
        assert_eq!((report.boosts, report.compiled_boosts), (2, 1));
        assert_eq!(report.current_repairs.skipped_ad_block_sites, 1);
        assert_eq!(report.last_good_repairs.skipped_ad_block_sites, 1);
        assert_eq!(report.before_import_repairs.skipped_ad_block_sites, 1);
        assert_eq!(fs::read(fixture.new.join(LEGACY_BOOSTS_FILE))?, boosts);
        assert_eq!(
            fs::read(fixture.new.join(LEGACY_COMPILED_BOOSTS_FILE))?,
            compiled
        );
        let state = fixture.load()?.unwrap();
        assert_session(&state, "current");
        if version == Some(2) {
            assert_eq!(state.windows()[0].id, WindowId::new("original"));
        }
        for (file, path) in [
            ("state.json", "current"),
            ("last-good.json", "previous"),
            ("before-import.json", "previous"),
        ] {
            let encoded: String =
                serde_json::from_slice(&fs::read(fixture.new.join("session").join(file))?)?;
            let json: Value = serde_json::from_str(&encoded)?;
            assert_eq!(json["version"], storage::SCHEMA_VERSION);
            assert_session(&storage::decode(&encoded)?, path);
        }
    }
    Ok(())
}

#[test]
fn falls_back_only_when_the_primary_key_exists() -> Result<()> {
    let last_good = snapshot(Some(2), "backup");
    let fixture = Fixture::write(&[("state", b"\"{\""), ("state:last-good", &last_good)])?;
    let report = fixture.migrate()?;
    assert_eq!(report.state_source, StateSource::LastGood);
    assert_eq!((report.migrated_keys, report.invalid_keys), (1, 1));
    assert_session(&fixture.load()?.unwrap(), "backup");

    let missing = Fixture::write(&[("state:last-good", &last_good)])?;
    assert_eq!(missing.migrate()?.state_source, StateSource::Fresh);
    assert_eq!(missing.load()?, None);
    assert!(missing.new.join("session/last-good.json").is_file());
    Ok(())
}

#[test]
fn reports_two_damaged_snapshots_and_keeps_valid_boosts() -> Result<()> {
    let fixture = Fixture::write(&[
        ("state", b"true"),
        ("state:last-good", b"bad outer JSON"),
        ("boosts", b"{\"all.css\":\"body {}\"}"),
        ("boostsCompiled", b"[]"),
    ])?;
    let report = fixture.migrate()?;
    assert_eq!(report.state_source, StateSource::Fresh);
    assert_eq!((report.migrated_keys, report.invalid_keys), (1, 3));
    assert_eq!(fixture.load()?, None);
    assert!(fixture.new.join(LEGACY_BOOSTS_FILE).is_file());
    assert!(!fixture.new.join(LEGACY_COMPILED_BOOSTS_FILE).exists());
    Ok(())
}

#[test]
fn refuses_live_and_remote_locks_and_leaves_stale_lock_untouched() -> Result<()> {
    let fixture = Fixture::write(&[("state", &snapshot(None, "current"))])?;
    let lock = fixture.old.join("SingletonLock");
    let uname = rustix::system::uname();
    let host = uname.nodename().to_str()?;
    let mut process = std::process::Command::new("true").spawn()?;
    let dead_pid = process.id();
    assert!(process.wait()?.success());
    for target in [
        format!("{host}-{}", std::process::id()),
        format!("other-{host}-{dead_pid}"),
        "malformed".into(),
    ] {
        symlink(target, &lock)?;
        let error = fixture.migrate().unwrap_err();
        assert!(error.is::<ProfileInUse>());
        assert!(!fixture.new.exists());
        fs::remove_file(&lock)?;
    }
    symlink(format!("{host}-{dead_pid}"), &lock)?;
    assert_eq!(fixture.migrate()?.state_source, StateSource::Primary);
    Ok(())
}

fn assert_database_error(fixture: &Fixture, reason: &str) {
    let error = fixture.migrate().unwrap_err();
    let error = format!("{error:#}");
    assert!(
        error.contains("LevelDB") && error.contains(reason),
        "{error}"
    );
    assert!(!fixture.new.exists());
}

#[test]
fn rejects_k1_corrupt_log_record() -> Result<()> {
    let fixture = Fixture::cpp_leveldb("logonly")?;
    let path = fixture.leveldb.join("000003.log");
    let mut bytes = fs::read(&path)?;
    let needle = b"https://example.test/current";
    let offset = bytes
        .windows(needle.len())
        .position(|value| value == needle)
        .unwrap();
    bytes[offset + 3] ^= 0x20;
    fs::write(path, bytes)?;
    assert_database_error(&fixture, "record checksum mismatch");
    Ok(())
}

#[test]
fn rejects_k2_corrupt_only_table() -> Result<()> {
    let fixture = Fixture::cpp_leveldb("snappy")?;
    flip_table_byte(&fixture, "000005.ldb", 496)?;
    assert_database_error(&fixture, "block checksum mismatch");
    Ok(())
}

#[test]
fn rejects_k3_corrupt_newer_table_instead_of_reading_old_state() -> Result<()> {
    let fixture = Fixture::cpp_leveldb("twolevel")?;
    flip_table_byte(&fixture, "000007.ldb", 435)?;
    assert_database_error(&fixture, "block checksum mismatch");
    Ok(())
}

#[test]
fn rejects_k4_truncated_table_without_panicking() -> Result<()> {
    let fixture = Fixture::cpp_leveldb("snappy")?;
    let path = fixture.leveldb.join("000005.ldb");
    let mut bytes = fs::read(&path)?;
    bytes.truncate(bytes.len() / 2);
    fs::write(path, bytes)?;
    assert_database_error(&fixture, "invalid table magic");
    Ok(())
}

#[test]
fn rejects_a_torn_final_write_ahead_log_record() -> Result<()> {
    let fixture = Fixture::cpp_leveldb("logonly")?;
    let log = fs::OpenOptions::new()
        .write(true)
        .open(fixture.leveldb.join("000003.log"))?;
    log.set_len(log.metadata()?.len() - 1)?;
    assert_database_error(&fixture, "truncated record payload");
    Ok(())
}

fn flip_table_byte(fixture: &Fixture, name: &str, first_data_block_size: usize) -> Result<()> {
    let path = fixture.leveldb.join(name);
    let mut bytes = fs::read(&path)?;
    assert_eq!(bytes[first_data_block_size], 1, "Snappy compression tag");
    bytes[first_data_block_size / 2] ^= 0xff;
    fs::write(path, bytes)?;
    Ok(())
}

#[test]
fn reads_undamaged_cpp_leveldb_fixtures() -> Result<()> {
    for (layout, expected) in [
        ("logonly", "current"),
        ("snappy", "current"),
        ("twolevel", "new"),
    ] {
        let fixture = Fixture::cpp_leveldb(layout)?;
        assert_eq!(fixture.migrate()?.state_source, StateSource::Primary);
        let state = fixture.load()?.unwrap();
        assert_eq!(
            state.tab(&TabId::new("t1")).unwrap().url,
            format!("https://example.test/{expected}")
        );
    }
    Ok(())
}

#[test]
fn rejects_database_links_and_manifest_paths() -> Result<()> {
    let fixture = Fixture::write(&[("state", &snapshot(None, "current"))])?;
    let outside = fixture.old.parent().unwrap().join("outside");
    fs::rename(&fixture.leveldb, &outside)?;
    symlink(&outside, &fixture.leveldb)?;
    let error = fixture.migrate().unwrap_err();
    assert_eq!(
        error.to_string(),
        "open Flecha shell extension database directory"
    );
    assert!(!fixture.new.exists());
    fs::remove_file(&fixture.leveldb)?;
    fs::rename(&outside, &fixture.leveldb)?;

    let current = fixture.leveldb.join("CURRENT");
    fs::rename(&current, &outside)?;
    symlink(&outside, &current)?;
    let error = fixture.migrate().unwrap_err();
    assert!(error.to_string().contains("must be a regular file"));
    fs::remove_file(&current)?;
    fs::hard_link(&outside, &current)?;
    let error = fixture.migrate().unwrap_err();
    assert!(error.to_string().contains("must be a regular file"));
    fs::remove_file(&current)?;
    assert!(!fixture.new.exists());

    fs::write(&current, b"../outside\n")?;
    let error = fixture.migrate().unwrap_err();
    assert!(error.to_string().contains("invalid manifest"));
    assert!(!fixture.new.exists());
    Ok(())
}

#[test]
fn never_writes_inside_the_source_or_replaces_an_existing_profile() -> Result<()> {
    let fixture = Fixture::write(&[("state", &snapshot(None, "current"))])?;
    let before = hash_tree(&fixture.old)?;
    let error = migrate_profile(
        &fixture.old,
        OsStr::new("Default"),
        &fixture.old.join("new"),
    )
    .unwrap_err();
    assert!(format!("{error:#}").contains("destination is inside the old profile"));
    assert_eq!(hash_tree(&fixture.old)?, before);

    fs::create_dir(&fixture.new)?;
    fs::write(fixture.new.join("keep"), b"existing")?;
    let error = fixture.migrate().unwrap_err();
    assert!(error.to_string().contains("destination already exists"));
    assert_eq!(fs::read(fixture.new.join("keep"))?, b"existing");
    Ok(())
}
