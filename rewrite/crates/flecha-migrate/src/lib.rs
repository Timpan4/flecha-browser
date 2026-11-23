//! Copy the Chromium Flecha shell's saved state into a new rewrite profile.

#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::DirBuilderExt as _;
use std::path::Path;

use anyhow::{Context as _, Result, anyhow};
use flecha_core::storage::{self, DecodedSnapshot, SnapshotRepairs};
use flecha_services::StateStore;
use rusty_leveldb::{DB, LdbIterator, Options};
use serde_json::Value;
use serde_json::value::RawValue;

mod profile;

/// Pinned by `chromium/patches/0004-flecha-shell-extension-id.patch`.
pub const SHELL_EXTENSION_ID: &str = "abdiaepfgbbhdjaecjipmaailijaaide";
pub const LEGACY_BOOSTS_FILE: &str = "legacy-boosts.json";
pub const LEGACY_COMPILED_BOOSTS_FILE: &str = "legacy-boosts-compiled.json";

/// Chromium is running or its singleton lock cannot be confirmed stale.
#[derive(Debug)]
pub struct ProfileInUse;

impl std::fmt::Display for ProfileInUse {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Chromium profile is in use or has an unverified SingletonLock")
    }
}

impl std::error::Error for ProfileInUse {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StateSource {
    Primary,
    LastGood,
    #[default]
    Fresh,
}

#[derive(Debug, Default, Eq, PartialEq)]
pub struct MigrationReport {
    pub state_source: StateSource,
    pub current_repairs: SnapshotRepairs,
    pub last_good_repairs: SnapshotRepairs,
    pub before_import_repairs: SnapshotRepairs,
    pub spaces: usize,
    pub windows: usize,
    pub open_tabs: usize,
    pub archive_entries: usize,
    pub ad_block_sites: usize,
    pub boosts: usize,
    pub compiled_boosts: usize,
    /// One count per input key consumed, including valid backups and Boosts files.
    pub migrated_keys: usize,
    pub invalid_keys: usize,
    pub unknown_keys: usize,
    /// `state:damaged` is retained in the source profile for manual recovery.
    pub damaged_backups: usize,
}

/// Migrate an explicitly selected Chromium profile. `user_data_dir` is the directory
/// containing Chromium's `SingletonLock`; `profile_name` is one directory name such as
/// `Default`. Supply absolute paths and an existing, trusted destination parent outside
/// `user_data_dir`. Keep Chromium closed until this returns.
///
/// The profile is published in one rename. An existing destination is an error.
pub fn migrate_profile(
    user_data_dir: &Path,
    profile_name: &OsStr,
    destination: &Path,
) -> Result<MigrationReport> {
    let source = profile::Source::open(user_data_dir, profile_name)?;
    let output = profile::Output::new(destination, &source)?;
    let db_path = output.scratch().join("extension-db");
    fs::create_dir(&db_path)?;
    source.copy_database(&db_path)?;

    let stage = output.scratch().join("profile");
    fs::DirBuilder::new().mode(0o700).create(&stage)?;
    let report = migrate_database(&db_path, &stage)?;
    output.publish()?;
    Ok(report)
}

fn migrate_database(db_path: &Path, destination: &Path) -> Result<MigrationReport> {
    let mut db = DB::open(
        db_path,
        Options {
            create_if_missing: false,
            paranoid_checks: true,
            ..Options::default()
        },
    )
    .map_err(|error| anyhow!("open copied extension LevelDB ({:?})", error.code))?;
    let mut entries = db
        .new_iter()
        .map_err(|error| anyhow!("read copied extension LevelDB ({:?})", error.code))?;
    let mut report = MigrationReport::default();
    let mut primary = None;
    let mut last_good = None;
    let mut before_import = None;
    let mut has_primary = false;

    let (store, mut loaded) = StateStore::open(&destination.join("session"))?;
    while entries.advance() {
        let (key, value) = entries.current().context("read extension LevelDB entry")?;
        match key.as_ref() {
            b"state" => {
                has_primary = true;
                primary = decode_state(&value, &mut report);
            }
            b"state:last-good" => last_good = decode_state(&value, &mut report),
            b"state:before-import" => before_import = decode_state(&value, &mut report),
            b"state:damaged" => report.damaged_backups += 1,
            b"boosts" => {
                report.boosts = carry_boosts(destination, LEGACY_BOOSTS_FILE, &value, &mut report)?;
            }
            b"boostsCompiled" => {
                report.compiled_boosts = carry_boosts(
                    destination,
                    LEGACY_COMPILED_BOOSTS_FILE,
                    &value,
                    &mut report,
                )?;
            }
            _ => report.unknown_keys += 1,
        }
    }
    let current = if primary.is_some() {
        report.state_source = StateSource::Primary;
        primary.as_ref()
    } else if has_primary && last_good.is_some() {
        report.state_source = StateSource::LastGood;
        last_good.as_ref()
    } else {
        None
    };
    if let Some(snapshot) = current {
        let state = &snapshot.state;
        let ready = loaded.apply_recovery(|write| store.write(write))?;
        store.write(&ready.save_current(state)?)?;
        report.current_repairs = snapshot.repairs;
        report.spaces = state.spaces().len();
        report.windows = state.windows().len();
        report.open_tabs = state.open_tabs().len();
        report.archive_entries = state.archive().len();
        report.ad_block_sites = state.settings.ad_block_allowed.as_ref().map_or(0, Vec::len);
    }
    if let Some(snapshot) = last_good {
        let value = Value::String(storage::encode(&snapshot.state)?);
        let mut backup = storage::load(Some(&value), None)?;
        let ready = backup.ready().context("prepare migrated last-good state")?;
        let write = ready.started().context("encode migrated last-good state")?;
        store.write(&write)?;
        report.last_good_repairs = snapshot.repairs;
    }
    if let Some(snapshot) = before_import {
        store.write(&storage::set_aside_before_import(&snapshot.state)?)?;
        report.before_import_repairs = snapshot.repairs;
    }
    Ok(report)
}

fn decode_state(value: &[u8], report: &mut MigrationReport) -> Option<DecodedSnapshot> {
    // Chrome stores the JSON representation of the string returned by Store.serialize().
    let decoded = serde_json::from_slice::<String>(value)
        .ok()
        .and_then(|json| storage::migrate_legacy_with_repairs(&json).ok());
    if decoded.is_some() {
        report.migrated_keys += 1;
    } else {
        report.invalid_keys += 1;
    }
    decoded
}

fn carry_boosts(
    destination: &Path,
    name: &str,
    value: &[u8],
    report: &mut MigrationReport,
) -> Result<usize> {
    let Ok(entries) = serde_json::from_slice::<BTreeMap<String, &RawValue>>(value) else {
        report.invalid_keys += 1;
        return Ok(0);
    };
    fs::write(destination.join(name), value).context("save legacy Boosts JSON")?;
    report.migrated_keys += 1;
    Ok(entries.len())
}
