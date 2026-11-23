//! JSON snapshots and recovery decisions. The application owns reads, writes and startup.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, value::RawValue};

mod legacy_pins;
mod snapshot;

use crate::input_text::trim_input;
use crate::shortcuts::repair_saved_bindings;
use crate::state::{DEFAULT_SPACE_COLOR, PinnedTree, compare_recency};
use crate::{
    ArchivePolicy, ArchivedTab, BrowserState, ClosedWindow, Device, Environment, EnvironmentTarget,
    EpochMillis, FolderId, PinnedNode, Settings, SidebarWidth, Space, SpaceId, Tab, TabId, Window,
    WindowId, WindowSpace, is_site_host,
};

/// The first Rust snapshot format. Versions 1 and 2 refer to the Chromium shell.
pub const SCHEMA_VERSION: u64 = 3;

#[derive(Debug)]
pub enum DecodeError {
    Json(serde_json::Error),
    InvalidShape,
    UnsupportedVersion(String),
    AlreadyMigrated,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Json(error) => write!(f, "invalid state JSON: {error}"),
            Self::InvalidShape => f.write_str("state has no usable Spaces or tab map"),
            Self::UnsupportedVersion(version) => write!(f, "unsupported state schema {version}"),
            Self::AlreadyMigrated => f.write_str("expected a Chromium shell state copy"),
        }
    }
}

impl std::error::Error for DecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}

pub fn encode(state: &BrowserState) -> serde_json::Result<String> {
    #[derive(Serialize)]
    struct Snapshot<'a> {
        version: u64,
        #[serde(flatten)]
        state: &'a BrowserState,
    }

    serde_json::to_string_pretty(&Snapshot {
        version: SCHEMA_VERSION,
        state,
    })
}

pub fn decode(json: &str) -> Result<BrowserState, DecodeError> {
    decode_with_repairs(json).map(|decoded| decoded.state)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SnapshotRepairs {
    /// Legacy folders flattened into the deepest retained ancestor.
    pub flattened_legacy_folders: usize,
    /// Saved ad-block exceptions dropped because they are not request-domain hostnames.
    pub skipped_ad_block_sites: usize,
}

#[derive(Debug)]
pub struct DecodedSnapshot {
    pub state: BrowserState,
    pub repairs: SnapshotRepairs,
}

pub fn decode_with_repairs(json: &str) -> Result<DecodedSnapshot, DecodeError> {
    decode_document(json, false)
}

/// Migrates the JSON state value from a copied Chromium profile or export.
pub fn migrate_legacy(copy: &str) -> Result<BrowserState, DecodeError> {
    migrate_legacy_with_repairs(copy).map(|decoded| decoded.state)
}

/// Migrates a Chromium profile copy and reports folder flattening to the caller.
pub fn migrate_legacy_with_repairs(copy: &str) -> Result<DecodedSnapshot, DecodeError> {
    decode_document(copy, true)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Slot {
    Current,
    LastGood,
    Damaged,
    BeforeImport,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BackupWrite {
    slot: Slot,
    value: Value,
}

impl BackupWrite {
    pub fn slot(&self) -> Slot {
        self.slot
    }

    pub fn value(&self) -> &Value {
        &self.value
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Recovery {
    LastGood,
    Fresh,
    /// The current snapshot is damaged and the last-good copy needs a newer reader.
    UnsupportedBackup,
}

/// Save this write before replacing the current state from a whole-state import.
pub fn set_aside_before_import(state: &BrowserState) -> serde_json::Result<BackupWrite> {
    Ok(BackupWrite {
        slot: Slot::BeforeImport,
        value: Value::String(encode(state)?),
    })
}

pub fn state_before_import(saved: Option<&Value>) -> Result<Option<BrowserState>, DecodeError> {
    saved
        .map_or(Ok(None), readable)
        .map(|decoded| decoded.map(|decoded| decoded.state))
}

/// Startup state whose current-slot writer is gated on recovery.
///
/// ```compile_fail
/// # use flecha_core::storage::load;
/// # use flecha_core::{BrowserState, SpaceId, WindowId};
/// let loaded = load(None, None).unwrap();
/// let state = BrowserState::new(SpaceId::new("s"), WindowId::new("w"));
/// loaded.save_current(&state).unwrap();
/// ```
#[derive(Debug)]
pub struct LoadedState {
    ready: ReadyState,
    damaged_copy: Option<BackupWrite>,
    recovered_current: Option<BackupWrite>,
}

#[derive(Debug)]
pub struct ReadyState {
    pub state: Option<BrowserState>,
    pub recovered: Option<Recovery>,
    pub repairs: SnapshotRepairs,
    last_good_after_start: Option<BackupWrite>,
}

impl LoadedState {
    /// Obtain the current-slot writer only after any recovery writes have succeeded.
    pub fn ready(&mut self) -> Option<&mut ReadyState> {
        if self.damaged_copy.is_some() || self.recovered_current.is_some() {
            return None;
        }
        Some(&mut self.ready)
    }

    /// Persist recovery writes. Return success from the writer only after the value is saved.
    /// On failure, another call resumes at the failed write.
    pub fn apply_recovery<E>(
        &mut self,
        mut write: impl FnMut(&BackupWrite) -> Result<(), E>,
    ) -> Result<&mut ReadyState, E> {
        if let Some(damaged) = &self.damaged_copy {
            write(damaged)?;
            self.damaged_copy = None;
        }
        if let Some(current) = &self.recovered_current {
            write(current)?;
            self.recovered_current = None;
        }
        Ok(&mut self.ready)
    }
}

impl ReadyState {
    pub fn save_current(&self, state: &BrowserState) -> serde_json::Result<BackupWrite> {
        Ok(BackupWrite {
            slot: Slot::Current,
            value: Value::String(encode(state)?),
        })
    }

    /// Promote the repaired startup snapshot only after the application starts successfully.
    /// This is the original startup state, regardless of subsequent runtime edits.
    pub fn started(&mut self) -> Option<BackupWrite> {
        self.last_good_after_start.take()
    }
}

/// Read the two stored JSON strings or retain the exact damaged value if it was not a string.
/// A newer schema is surfaced as an error so an older binary cannot overwrite it.
pub fn load(saved: Option<&Value>, last_good: Option<&Value>) -> Result<LoadedState, DecodeError> {
    let mut loaded = LoadedState {
        ready: ReadyState {
            state: None,
            recovered: None,
            repairs: SnapshotRepairs::default(),
            last_good_after_start: None,
        },
        damaged_copy: None,
        recovered_current: None,
    };
    let Some(saved) = saved else {
        return Ok(loaded);
    };
    if let Some(decoded) = readable(saved)? {
        let repaired = Value::String(encode(&decoded.state).map_err(DecodeError::Json)?);
        if last_good != Some(&repaired) && !last_good.is_some_and(needs_newer_reader) {
            loaded.ready.last_good_after_start = Some(BackupWrite {
                slot: Slot::LastGood,
                value: repaired,
            });
        }
        loaded.ready.state = Some(decoded.state);
        loaded.ready.repairs = decoded.repairs;
        return Ok(loaded);
    }

    loaded.damaged_copy = Some(BackupWrite {
        slot: Slot::Damaged,
        value: saved.clone(),
    });
    match last_good.map_or(Ok(None), readable) {
        Ok(Some(fallback)) => {
            loaded.ready.state = Some(fallback.state);
            loaded.ready.repairs = fallback.repairs;
            loaded.ready.recovered = Some(Recovery::LastGood);
            loaded.recovered_current = Some(BackupWrite {
                slot: Slot::Current,
                value: last_good
                    .expect("a fallback comes from the last-good copy")
                    .clone(),
            });
        }
        Ok(None) => loaded.ready.recovered = Some(Recovery::Fresh),
        Err(DecodeError::UnsupportedVersion(_)) => {
            loaded.ready.recovered = Some(Recovery::UnsupportedBackup)
        }
        Err(error) => return Err(error),
    }
    Ok(loaded)
}

fn readable(value: &Value) -> Result<Option<DecodedSnapshot>, DecodeError> {
    match value.as_str().map(decode_with_repairs) {
        Some(Ok(state)) => Ok(Some(state)),
        Some(Err(error @ DecodeError::UnsupportedVersion(_))) => Err(error),
        _ => Ok(None),
    }
}

fn decode_document(json: &str, legacy_only: bool) -> Result<DecodedSnapshot, DecodeError> {
    let fields = snapshot::read_record(json).map_err(DecodeError::Json)?;
    if let Some(version) = fields.get("version") {
        check_version(version, legacy_only)?;
    }
    let tabs = fields.get("tabs").ok_or(DecodeError::InvalidShape)?;
    let tabs = snapshot::read_record(tabs.get()).map_err(|_| DecodeError::InvalidShape)?;
    let tabs = tabs
        .into_iter()
        .filter_map(|(id, tab)| Some((TabId::new(id), repair_tab(tab)?)))
        .collect();
    let mut repairs = SnapshotRepairs::default();
    let projected = snapshot::project(&fields, &mut repairs)?;
    let state = repair(projected, tabs, &mut repairs).ok_or(DecodeError::InvalidShape)?;
    Ok(DecodedSnapshot { state, repairs })
}

fn check_version(version: &RawValue, legacy_only: bool) -> Result<(), DecodeError> {
    match version_order(version, SCHEMA_VERSION) {
        Some(StoredVersion {
            order: Ordering::Greater,
            text,
        }) => Err(DecodeError::UnsupportedVersion(text)),
        Some(StoredVersion {
            order: Ordering::Equal,
            ..
        }) if legacy_only => Err(DecodeError::AlreadyMigrated),
        // The Chromium decoder accepted older values and ignored nonnumeric tags.
        _ => Ok(()),
    }
}

/// A stored document's `version`, compared with the reader's schema version.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredVersion {
    /// The version as stored, with a string's quotes and surrounding whitespace removed.
    pub text: String,
    pub order: Ordering,
}

/// Read only the `version` of a JSON object document and compare it with `current`.
///
/// Nothing else in the document is decoded, so a newer document is recognised whatever its other
/// fields hold. A number or numeric string compares by its decimal spelling. A missing or
/// nonnumeric version gives `None`. An error means the text is not a JSON object.
pub fn read_schema_version(json: &str, current: u64) -> serde_json::Result<Option<StoredVersion>> {
    let fields = snapshot::read_record(json)?;
    Ok(fields
        .get("version")
        .and_then(|version| version_order(version, current)))
}

fn version_order(version: &RawValue, current: u64) -> Option<StoredVersion> {
    let raw = version.get();
    let decoded;
    let text = if raw.starts_with('"') {
        decoded = snapshot::scalar(version);
        trim_input(decoded.as_str().unwrap_or_default())
    } else {
        raw
    };
    compare_version(text, &current.to_string()).map(|order| StoredVersion {
        text: text.into(),
        order,
    })
}

/// Compare the decimal spelling without rounding a newer schema to the current version.
fn compare_version(text: &str, current: &str) -> Option<Ordering> {
    let (mantissa, exp) = if let Some((mantissa, exp)) = text.split_once(['e', 'E']) {
        let (negative, digits) = sign_and_digits(exp);
        if digits.is_empty() || !digits.bytes().all(|digit| digit.is_ascii_digit()) {
            return None;
        }
        // An exponent outside i128's range dominates any significand that fits in memory.
        let exp = exp
            .parse::<i128>()
            .unwrap_or(if negative { i128::MIN } else { i128::MAX });
        (mantissa, exp)
    } else {
        (text, 0)
    };
    let (negative, mantissa) = sign_and_digits(mantissa);
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = whole.bytes().chain(fraction.bytes());
    if mantissa.is_empty() || !digits.clone().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let zeroes = digits.clone().take_while(|&digit| digit == b'0').count();
    if negative || zeroes == whole.len() + fraction.len() {
        return Some(Ordering::Less);
    }
    let width = (whole.len() as i128 - zeroes as i128).saturating_add(exp);
    if width != current.len() as i128 {
        return Some(width.cmp(&(current.len() as i128)));
    }
    let mut digits = digits.skip(zeroes);
    for expected in current.bytes() {
        let digit = digits.next().unwrap_or(b'0');
        if digit != expected {
            return Some(digit.cmp(&expected));
        }
    }
    Some(if digits.any(|digit| digit != b'0') {
        Ordering::Greater
    } else {
        Ordering::Equal
    })
}

fn sign_and_digits(text: &str) -> (bool, &str) {
    (
        text.starts_with('-'),
        text.strip_prefix(['+', '-']).unwrap_or(text),
    )
}

fn needs_newer_reader(value: &Value) -> bool {
    let fields: Option<BTreeMap<String, &RawValue>> = value
        .as_str()
        .and_then(|json| snapshot::read_record(json).ok());
    fields
        .and_then(|fields| fields.get("version").copied())
        .is_some_and(|version| {
            matches!(
                check_version(version, false),
                Err(DecodeError::UnsupportedVersion(_))
            )
        })
}

fn repair(
    mut raw: Map<String, Value>,
    tabs: BTreeMap<TabId, Tab>,
    repairs: &mut SnapshotRepairs,
) -> Option<BrowserState> {
    let raw_spaces = raw.remove("spaces")?;
    let raw_spaces: Vec<_> = raw_spaces
        .as_array()?
        .iter()
        .filter_map(|space| Some((space.get("id")?.as_str()?, space)))
        .collect();
    let mut referenced: BTreeSet<_> = raw_spaces.iter().map(|(id, _)| *id).collect();
    referenced.extend(raw.get("activeSpaceId").and_then(Value::as_str));
    for window in list(raw.get("windows")).chain(list(raw.get("closedWindows"))) {
        referenced.extend(window.get("activeSpaceId").and_then(Value::as_str));
        if let Some(spaces) = window.get("spaces").and_then(Value::as_object) {
            referenced.extend(spaces.keys().map(String::as_str));
        }
    }
    let mut ids = RecoveredIds::new("space", referenced);
    let raw_spaces: Vec<_> = raw_spaces
        .into_iter()
        .map(|(id, space)| (SpaceId::new(ids.claim(Some(id))), space))
        .collect();
    let space_ids = raw_spaces.iter().map(|(id, _)| id.clone()).collect();
    let first_space = raw_spaces.first()?.0.clone();
    let mut placed = BTreeSet::new();
    let mut claim = |value: &Value| {
        let id = TabId::new(value.as_str()?);
        (tabs.contains_key(&id) && placed.insert(id.clone())).then_some(id)
    };
    let favorites = list(raw.get("favorites")).filter_map(&mut claim).collect();
    let mut folder_ids = BTreeSet::new();
    for (_, space) in &raw_spaces {
        collect_folder_ids(space.get("pinned"), &mut folder_ids);
        collect_folder_ids(space.get("pinnedFlat"), &mut folder_ids);
    }
    let mut folders = RecoveredIds::new("folder", folder_ids);
    let spaces = raw_spaces
        .iter()
        .map(|(id, space)| repair_space(id, space, &mut folders, &mut claim))
        .collect();

    let legacy;
    let raw_windows = match raw.get("windows").and_then(Value::as_array) {
        Some(windows) => windows.as_slice(),
        None => {
            let spaces = raw_spaces
                .iter()
                .map(|(id, space)| (id.as_str().into(), (*space).clone()))
                .collect();
            let mut win = Map::new();
            win.insert("spaces".into(), Value::Object(spaces));
            if let Some(active) = raw.get("activeSpaceId") {
                win.insert("activeSpaceId".into(), active.clone());
            }
            legacy = [Value::Object(win)];
            &legacy
        }
    };
    let input_closed = || {
        list(raw.get("closedWindows")).filter_map(|win| Some((millis(win.get("closedAt")?)?, win)))
    };
    let reserved: BTreeSet<_> = raw_windows
        .iter()
        .chain(input_closed().map(|(_, win)| win))
        .filter_map(|win| win.get("id")?.as_str())
        .collect();
    let mut window_ids = RecoveredIds::new("window", reserved);
    let mut window = |value: &Value| {
        let id = WindowId::new(window_ids.claim(value.get("id").and_then(Value::as_str)));
        repair_window(value, id, &space_ids, &first_space, &mut claim)
    };
    let mut windows: Vec<_> = raw_windows
        .iter()
        .filter(|value| value.is_object())
        .map(&mut window)
        .collect();
    if windows.is_empty() {
        windows.push(window(&Value::Null));
    }
    let closed_windows = input_closed()
        .map(|(closed_at, value)| ClosedWindow {
            window: window(value),
            closed_at,
        })
        .filter(|closed| !closed.window.spaces.is_empty())
        .collect();
    let archive = list(raw.get("archive"))
        .filter_map(|entry| {
            Some(ArchivedTab {
                url: entry.get("url")?.as_str()?.into(),
                title: entry.get("title")?.as_str()?.into(),
                closed_at: millis(entry.get("closedAt")?)?,
            })
        })
        .collect();
    let sidebar_visible = raw
        .get("sidebarVisible")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let settings = repair_settings(&mut raw, repairs);
    let mut state = BrowserState {
        spaces,
        windows,
        closed_windows,
        favorites,
        tabs,
        archive,
        sidebar_visible,
        settings,
    };
    state.repair_selections();
    normalize_mobile(&mut state);
    state.repair_visible_tabs();
    Some(state)
}

type RawRecord<'a> = BTreeMap<String, &'a RawValue>;

fn raw_field<T: DeserializeOwned>(raw: &RawRecord<'_>, key: &str) -> Option<T> {
    serde_json::from_value(snapshot::scalar(raw.get(key)?)).ok()
}

fn repair_tab(raw: &RawValue) -> Option<Tab> {
    // Read only whitelisted fields. RawValue skips unknown nested data without
    // building a recursive tree or hitting the Value deserializer's depth limit.
    let raw = snapshot::read_record(raw.get()).ok()?;
    let mut tab = Tab::new(raw_field::<String>(&raw, "url")?, 0);
    if let Some(title) = raw_field(&raw, "title") {
        tab.title = title;
    }
    tab.last_active = raw_field(&raw, "lastActive")
        .as_ref()
        .and_then(coerce_millis);
    tab.favicon = raw_field(&raw, "favicon");
    tab.custom_title = raw_field(&raw, "customTitle");
    tab.muted = raw_field(&raw, "muted");
    tab.device = raw_field(&raw, "device");
    tab.home_url = raw_field(&raw, "homeUrl");
    tab.unseen = raw_field(&raw, "unseen").unwrap_or_default();
    tab.agent = raw_field(&raw, "agent").unwrap_or_default();
    tab.history = raw
        .get("history")
        .and_then(|history| repair_history(history));
    Some(tab)
}

fn repair_history(raw: &RawValue) -> Option<crate::TabHistory> {
    let saved = snapshot::read_record(raw.get()).ok()?;
    let index = raw_field(&saved, "index")?;
    let entries: Vec<&RawValue> = serde_json::from_str(saved.get("entries")?.get()).ok()?;
    let entries = entries
        .into_iter()
        .map(|entry| {
            let entry = snapshot::read_record(entry.get()).ok()?;
            if entry
                .get("pageState")
                .is_some_and(|value| !value.get().starts_with('"'))
            {
                return None;
            }
            Some(crate::HistoryEntry {
                url: raw_field(&entry, "url")?,
                title: raw_field(&entry, "title")?,
            })
        })
        .collect::<Option<_>>()?;
    Some(crate::TabHistory { entries, index })
}

fn repair_space(
    id: &SpaceId,
    raw: &Value,
    folders: &mut RecoveredIds<'_>,
    claim: &mut impl FnMut(&Value) -> Option<TabId>,
) -> Space {
    Space {
        id: id.clone(),
        name: field(raw, "name").unwrap_or_else(|| "Space".into()),
        emoji: field(raw, "emoji"),
        color: field(raw, "color").unwrap_or_else(|| DEFAULT_SPACE_COLOR.into()),
        isolated: field(raw, "isolated").unwrap_or_default(),
        pinned_collapsed: field(raw, "pinnedCollapsed").unwrap_or_default(),
        pinned: repair_flat_pins(
            raw.get("pinned")
                .filter(|value| value.is_array())
                .or_else(|| raw.get("pinnedFlat")),
            folders,
            claim,
        ),
    }
}

fn repair_flat_pins(
    raw: Option<&Value>,
    folders: &mut RecoveredIds<'_>,
    claim: &mut impl FnMut(&Value) -> Option<TabId>,
) -> Vec<PinnedNode> {
    let mut tree = PinnedTree::default();
    for value in list(raw) {
        if let Some(depth) = field::<usize>(value, "depth")
            && tree.accepts_depth(depth)
        {
            tree.ascend(depth);
            if let Some(node) = repair_pin(value, folders, claim) {
                tree.push(depth, node);
            }
        }
    }
    tree.finish()
}

fn repair_pin(
    node: &Value,
    folders: &mut RecoveredIds<'_>,
    claim: &mut impl FnMut(&Value) -> Option<TabId>,
) -> Option<PinnedNode> {
    match node.get("kind")?.as_str()? {
        "tab" => Some(PinnedNode::Tab {
            tab_id: claim(node.get("tabId")?)?,
        }),
        "folder" => Some(PinnedNode::Folder {
            id: FolderId::new(folders.claim(Some(node.get("id")?.as_str()?))),
            name: field(node, "name").unwrap_or_else(|| "Folder".into()),
            open: field(node, "open").unwrap_or_default(),
            children: Vec::new(),
        }),
        _ => None,
    }
}

struct RecoveredIds<'a> {
    kind: &'static str,
    reserved: BTreeSet<&'a str>,
    seen: BTreeSet<String>,
    sequence: usize,
}

impl<'a> RecoveredIds<'a> {
    fn new(kind: &'static str, reserved: BTreeSet<&'a str>) -> Self {
        Self {
            kind,
            reserved,
            seen: BTreeSet::new(),
            sequence: 0,
        }
    }

    fn claim(&mut self, id: Option<&str>) -> String {
        if let Some(id) = id
            && self.seen.insert(id.into())
        {
            return id.into();
        }
        loop {
            self.sequence += 1;
            let id = format!("recovered-{}-{}", self.kind, self.sequence);
            if !self.reserved.contains(id.as_str()) {
                return id;
            }
        }
    }
}

fn collect_folder_ids<'a>(raw: Option<&'a Value>, ids: &mut BTreeSet<&'a str>) {
    for node in list(raw) {
        if node.get("kind").and_then(Value::as_str) == Some("folder")
            && let Some(id) = node.get("id").and_then(Value::as_str)
        {
            ids.insert(id);
        }
    }
}

fn repair_window(
    raw: &Value,
    id: WindowId,
    spaces: &BTreeSet<SpaceId>,
    first: &SpaceId,
    claim: &mut impl FnMut(&Value) -> Option<TabId>,
) -> Window {
    let active_space_id = field::<SpaceId>(raw, "activeSpaceId")
        .filter(|id| spaces.contains(id))
        .unwrap_or_else(|| first.clone());
    let spaces = raw
        .get("spaces")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(id, ws)| {
            let id = SpaceId::new(id);
            if !spaces.contains(&id) || !ws.is_object() {
                return None;
            }
            Some((
                id,
                WindowSpace {
                    today: list(ws.get("today")).filter_map(&mut *claim).collect(),
                    active_tab_id: field(ws, "activeTabId"),
                    split: field(ws, "split"),
                    split_mode: field(ws, "splitMode"),
                    split_ratio: field::<f64>(ws, "splitRatio")
                        .filter(|ratio| *ratio > 0.0 && *ratio < 1.0),
                },
            ))
        })
        .collect();
    Window {
        id,
        active_space_id,
        spaces,
        sidebar_visible: field(raw, "sidebarVisible"),
    }
}

fn normalize_mobile(state: &mut BrowserState) {
    let mut mirrors = BTreeSet::new();
    for ws in state
        .windows
        .iter_mut()
        .flat_map(|win| win.spaces.values_mut())
    {
        if ws.split_mode.is_none() {
            continue;
        }
        if let Some([_, phone]) = &ws.split
            && ws.today.contains(phone)
            && state
                .tabs
                .get(phone)
                .is_some_and(|tab| tab.device == Some(Device::Mobile))
        {
            mirrors.insert(phone.clone());
        } else {
            ws.split_mode = None;
        }
    }
    for closed in &mut state.closed_windows {
        closed.window.spaces.retain(|_, ws| {
            if ws.split_mode.take().is_some()
                && let Some([desktop, phone]) = ws.split.take()
                && ws.today.contains(&phone)
                && state
                    .tabs
                    .get(&phone)
                    .is_some_and(|tab| tab.device == Some(Device::Mobile))
            {
                ws.today.retain(|id| *id != phone);
                state.tabs.remove(&phone);
                if ws.active_tab_id == Some(phone) && ws.today.contains(&desktop) {
                    ws.active_tab_id = Some(desktop);
                }
            }
            if ws
                .active_tab_id
                .as_ref()
                .is_none_or(|id| !ws.today.contains(id))
            {
                ws.active_tab_id = ws
                    .today
                    .iter()
                    .min_by(|left, right| compare_recency(&state.tabs, left, right))
                    .cloned();
            }
            !ws.today.is_empty()
        });
    }
    state
        .closed_windows
        .retain(|closed| !closed.window.spaces.is_empty());
    for (id, tab) in &mut state.tabs {
        if !mirrors.contains(id) {
            tab.device = None;
        }
    }
}

fn repair_settings(raw: &mut Map<String, Value>, repairs: &mut SnapshotRepairs) -> Settings {
    fn take<T: DeserializeOwned>(raw: &mut Map<String, Value>, key: &str) -> Option<T> {
        serde_json::from_value(raw.remove(key)?).ok()
    }

    let mut keybindings = take(raw, "keybindings").unwrap_or_default();
    repair_saved_bindings(&mut keybindings);
    Settings {
        sidebar_width: take::<f64>(raw, "sidebarWidth")
            .and_then(SidebarWidth::from_saved)
            .unwrap_or_default(),
        theme: take(raw, "theme").unwrap_or_default(),
        search_engine: take(raw, "searchEngine").unwrap_or_default(),
        archive_policy: take(raw, "archiveAfterHours")
            .and_then(ArchivePolicy::from_hours)
            .unwrap_or_default(),
        keybindings,
        environments: raw
            .remove("environments")
            .and_then(repair_environments)
            .unwrap_or_default(),
        window: raw
            .get("window")
            .filter(|window| window.get("maximized").is_none_or(Value::is_boolean))
            .and_then(|value| serde_json::from_value(value.clone()).ok()),
        peek_pinned_links: take(raw, "peekPinnedLinks"),
        open_links_in_flechinha: take(raw, "openLinksInFlechinha"),
        sidebar_in_page_fullscreen: take(raw, "sidebarInPageFullscreen"),
        downloads_folder: take(raw, "downloadsFolder"),
        ask_where_to_save: take(raw, "askWhereToSave"),
        ad_block: take(raw, "adBlock"),
        ad_block_allowed: repair_ad_block_sites(raw.remove("adBlockAllowed"), repairs),
        onboarded: take(raw, "onboarded"),
        site_permissions: take(raw, "sitePermissions"),
    }
}

/// Keeps every saved exception that is a request-domain hostname. The Chromium shell drops the
/// whole list for one bad entry because declarativeNetRequest refuses the whole allow rule; the
/// rewrite's engine checks entries one by one, so one bad entry costs only itself.
fn repair_ad_block_sites(raw: Option<Value>, repairs: &mut SnapshotRepairs) -> Option<Vec<String>> {
    let Value::Array(entries) = raw? else {
        return None;
    };
    let total = entries.len();
    let sites: Vec<String> = entries
        .into_iter()
        .filter_map(|entry| match entry {
            Value::String(host) if is_site_host(&host) => Some(host),
            _ => None,
        })
        .collect();
    repairs.skipped_ad_block_sites += total - sites.len();
    Some(sites)
}

fn repair_environments(raw: Value) -> Option<Vec<Environment>> {
    raw.as_array()?
        .iter()
        .map(|env| {
            Some(Environment {
                id: field(env, "id")?,
                name: field(env, "name")?,
                targets: env
                    .get("targets")?
                    .as_array()?
                    .iter()
                    .map(|target| {
                        Some(EnvironmentTarget {
                            label: field(target, "label")?,
                            origin: field(target, "origin")?,
                            auto_label: field(target, "autoLabel"),
                        })
                    })
                    .collect::<Option<_>>()?,
            })
        })
        .collect()
}

fn list(value: Option<&Value>) -> impl Iterator<Item = &Value> {
    value.and_then(Value::as_array).into_iter().flatten()
}

fn field<T: DeserializeOwned>(value: &Value, key: &str) -> Option<T> {
    T::deserialize(value.get(key)?).ok()
}

fn millis(value: &Value) -> Option<EpochMillis> {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|time| time as EpochMillis))
}

fn coerce_millis(value: &Value) -> Option<f64> {
    if let Some(text) = value.as_str() {
        let text = trim_input(text);
        for (prefix, radix) in [("0x", 16), ("0b", 2), ("0o", 8)] {
            if let Some(digits) = text
                .strip_prefix(prefix)
                .or_else(|| text.strip_prefix(&prefix.to_ascii_uppercase()))
            {
                return u64::from_str_radix(digits, radix)
                    .ok()
                    .map(|time| time as f64);
            }
        }
        return text.parse::<f64>().ok().filter(|time| time.is_finite());
    }
    value.as_f64().filter(|time| time.is_finite())
}
