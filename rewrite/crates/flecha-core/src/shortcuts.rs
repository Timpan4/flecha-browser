//! Platform defaults and conflict handling for Electron-style saved accelerators.

use std::collections::BTreeMap;
use std::sync::LazyLock;

pub type Keybindings = BTreeMap<String, Option<String>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Platform {
    Linux,
    Mac,
    Windows,
}

#[derive(Clone, Copy, Debug)]
pub struct Shortcut {
    pub id: &'static str,
    pub group: &'static str,
    pub label: &'static str,
    pub keys: &'static str,
}

const fn shortcut(
    id: &'static str,
    group: &'static str,
    label: &'static str,
    keys: &'static str,
) -> Shortcut {
    Shortcut {
        id,
        group,
        label,
        keys,
    }
}

pub const SHORTCUTS: &[Shortcut] = &[
    shortcut("newTab", "Getting around", "New tab", "CmdOrCtrl+T"),
    shortcut(
        "openLocation",
        "Getting around",
        "Open location",
        "CmdOrCtrl+L",
    ),
    shortcut("back", "Getting around", "Back", "CmdOrCtrl+["),
    shortcut("forward", "Getting around", "Forward", "CmdOrCtrl+]"),
    shortcut("reload", "Getting around", "Reload", "CmdOrCtrl+R"),
    shortcut(
        "hardReload",
        "Getting around",
        "Hard reload",
        "CmdOrCtrl+Shift+R",
    ),
    shortcut("find", "Getting around", "Find in page", "CmdOrCtrl+F"),
    shortcut(
        "toggleSidebar",
        "Getting around",
        "Show or hide the sidebar",
        "CmdOrCtrl+S",
    ),
    shortcut("settings", "Getting around", "Settings", "CmdOrCtrl+,"),
    shortcut("history", "Getting around", "History", "CmdOrCtrl+Y"),
    shortcut(
        "newFlechinha",
        "Tabs",
        "New Flechinha window",
        "Alt+CmdOrCtrl+N",
    ),
    shortcut("closeTab", "Tabs", "Close tab", "CmdOrCtrl+W"),
    shortcut(
        "reopenTab",
        "Tabs",
        "Reopen closed tab",
        "CmdOrCtrl+Shift+T",
    ),
    shortcut("nextTab", "Tabs", "Switch to recent tab", "Ctrl+Tab"),
    shortcut(
        "previousTab",
        "Tabs",
        "Switch to recent tab, backwards",
        "Ctrl+Shift+Tab",
    ),
    shortcut("pinTab", "Tabs", "Pin or unpin tab", "CmdOrCtrl+D"),
    shortcut("duplicateTab", "Tabs", "Duplicate tab", "Alt+CmdOrCtrl+D"),
    shortcut("muteTab", "Tabs", "Mute or unmute tab", "Ctrl+M"),
    shortcut("clearToday", "Tabs", "Clear Today", "CmdOrCtrl+Shift+K"),
    shortcut("copyLink", "Tabs", "Copy link", "CmdOrCtrl+Shift+C"),
    shortcut(
        "copyMarkdown",
        "Tabs",
        "Copy as Markdown link",
        "Alt+CmdOrCtrl+C",
    ),
    shortcut("screenshot", "Tabs", "Copy screenshot", "CmdOrCtrl+Shift+2"),
    shortcut("nextSpace", "Spaces", "Next Space", "Alt+CmdOrCtrl+Right"),
    shortcut(
        "previousSpace",
        "Spaces",
        "Previous Space",
        "Alt+CmdOrCtrl+Left",
    ),
    shortcut("newSpace", "Spaces", "New Space", "Ctrl+Shift+N"),
    shortcut(
        "splitNew",
        "Split & Peek",
        "Split with a new tab",
        "Ctrl+Shift+=",
    ),
    shortcut(
        "exitSplit",
        "Split & Peek",
        "Exit split view",
        "Ctrl+Shift+-",
    ),
    shortcut("zoomIn", "View", "Zoom in", "CmdOrCtrl+="),
    shortcut("zoomOut", "View", "Zoom out", "CmdOrCtrl+-"),
    shortcut("actualSize", "View", "Actual size", "CmdOrCtrl+0"),
    shortcut(
        "devTools",
        "Developer",
        "Developer Tools",
        "Alt+CmdOrCtrl+I",
    ),
    shortcut(
        "responsive",
        "Developer",
        "Responsive view",
        "Alt+CmdOrCtrl+M",
    ),
    shortcut(
        "nextEnvironment",
        "Developer",
        "Next environment",
        "Alt+CmdOrCtrl+E",
    ),
    shortcut(
        "editBoost",
        "Developer",
        "Edit boost for this site",
        "Alt+CmdOrCtrl+B",
    ),
    shortcut("viewSource", "Developer", "View source", "Alt+CmdOrCtrl+U"),
    shortcut(
        "shortcuts",
        "Getting around",
        "Keyboard shortcuts",
        "CmdOrCtrl+/",
    ),
    shortcut("quit", "Getting around", "Quit Flecha", "Ctrl+Shift+Q"),
];

pub fn default_keys(id: &str, platform: Platform) -> Option<&'static str> {
    match (id, platform) {
        ("newSpace", Platform::Linux | Platform::Windows) => Some("Ctrl+Alt+Shift+N"),
        ("history", Platform::Linux | Platform::Windows) => Some("Ctrl+H"),
        ("quit", Platform::Mac) => None,
        _ => SHORTCUTS
            .iter()
            .find(|command| command.id == id)
            .map(|command| command.keys),
    }
}

pub fn fixed_shortcuts(platform: Platform) -> BTreeMap<String, String> {
    let modifier = if platform == Platform::Mac {
        "Ctrl"
    } else {
        "Ctrl+Alt"
    };
    let mut fixed = BTreeMap::from([("CmdOrCtrl+O".into(), "openInFlecha".into())]);
    // The sidebar and Chromium register tab and Space selection for digits 1 through 9.
    for number in 1..=9 {
        fixed.insert(format!("CmdOrCtrl+{number}"), format!("selectTab:{number}"));
        fixed.insert(
            format!("{modifier}+{number}"),
            format!("selectSpace:{number}"),
        );
    }
    fixed
}

pub fn keys_for<'a>(bindings: &'a Keybindings, id: &str, platform: Platform) -> Option<&'a str> {
    let index = SHORTCUTS.iter().position(|command| command.id == id)?;
    let configured = configured_keys(bindings, id, platform)?;
    if parse_accelerator(configured, platform).is_some_and(|keys| is_reserved(keys, platform)) {
        return None;
    }
    let competing = |command: &Shortcut| {
        custom_keys(bindings, command.id, platform)
            .is_some_and(|keys| same_keys(keys, configured, platform))
    };
    let conflict = if custom_keys(bindings, id, platform).is_some() {
        // The shell overwrites identical map keys in declaration order. Chromium then
        // visits distinct keys in dictionary order and overwrites by parsed accelerator.
        SHORTCUTS.iter().enumerate().any(|(other, command)| {
            custom_keys(bindings, command.id, platform).is_some_and(|keys| {
                (keys, other) > (configured, index) && same_keys(keys, configured, platform)
            })
        })
    } else {
        SHORTCUTS
            .iter()
            .filter(|command| command.id != id)
            .any(competing)
    };
    (!conflict).then_some(configured)
}

fn custom_keys<'a>(bindings: &'a Keybindings, id: &str, platform: Platform) -> Option<&'a str> {
    bindings.get(id).and_then(Option::as_deref).filter(|keys| {
        assignable_accelerator(keys, platform)
            .is_some_and(|accelerator| !is_reserved(accelerator, platform))
    })
}

fn configured_keys<'a>(bindings: &'a Keybindings, id: &str, platform: Platform) -> Option<&'a str> {
    match bindings.get(id) {
        Some(None) => None,
        _ => custom_keys(bindings, id, platform).or_else(|| default_keys(id, platform)),
    }
}

fn assignable_accelerator(keys: &str, platform: Platform) -> Option<Accelerator> {
    parse_accelerator(keys, platform).filter(|accelerator| {
        accelerator.modifiers != 0 || matches!(accelerator.key, Key::Function(_))
    })
}

pub(crate) fn repair_saved_bindings(bindings: &mut Keybindings) {
    // The platform changes the CmdOrCtrl bit, but never whether a key is assignable.
    bindings.retain(|_, keys| {
        keys.as_deref()
            .is_none_or(|keys| assignable_accelerator(keys, Platform::Linux).is_some())
    });
}

fn is_reserved(keys: Accelerator, platform: Platform) -> bool {
    fn parsed_fixed_shortcuts(platform: Platform) -> Vec<Accelerator> {
        fixed_shortcuts(platform)
            .keys()
            .map(|keys| parse_accelerator(keys, platform).expect("fixed shortcut is valid"))
            .collect()
    }

    static MAC: LazyLock<Vec<Accelerator>> =
        LazyLock::new(|| parsed_fixed_shortcuts(Platform::Mac));
    static OTHER: LazyLock<Vec<Accelerator>> =
        LazyLock::new(|| parsed_fixed_shortcuts(Platform::Linux));
    let reserved = match platform {
        Platform::Mac => &MAC,
        Platform::Linux | Platform::Windows => &OTHER,
    };
    reserved.contains(&keys)
}

pub fn shortcut_map(bindings: &Keybindings, platform: Platform) -> BTreeMap<String, String> {
    let mut map = fixed_shortcuts(platform);
    for command in SHORTCUTS {
        if let Some(keys) = keys_for(bindings, command.id, platform) {
            map.insert(keys.into(), command.id.into());
        }
    }
    map
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingEdit<'a> {
    Reset,
    Unbind,
    Set(&'a str),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BindingError {
    UnknownCommand,
    InvalidAccelerator,
    Reserved,
}

/// Applies the edit and returns the IDs of commands that lost their bindings.
pub fn set_binding(
    bindings: &mut Keybindings,
    id: &str,
    edit: BindingEdit<'_>,
    platform: Platform,
) -> Result<Vec<&'static str>, BindingError> {
    if !SHORTCUTS.iter().any(|command| command.id == id) {
        return Err(BindingError::UnknownCommand);
    }
    let default = default_keys(id, platform);
    let keys = match edit {
        BindingEdit::Reset => default,
        BindingEdit::Unbind => {
            bindings.insert(id.into(), None);
            return Ok(Vec::new());
        }
        BindingEdit::Set(keys) => {
            if assignable_accelerator(keys, platform).is_none() {
                return Err(BindingError::InvalidAccelerator);
            }
            Some(keys)
        }
    };
    let mut displaced = Vec::new();
    if let Some(keys) = keys {
        if parse_accelerator(keys, platform).is_some_and(|keys| is_reserved(keys, platform)) {
            return Err(BindingError::Reserved);
        }
        for command in SHORTCUTS.iter().filter(|command| command.id != id) {
            let occupied = configured_keys(bindings, command.id, platform)
                .is_some_and(|current| same_keys(current, keys, platform));
            if occupied {
                bindings.insert(command.id.into(), None);
                displaced.push(command.id);
            }
        }
        if default.is_none_or(|fallback| !same_keys(keys, fallback, platform)) {
            bindings.insert(id.into(), Some(keys.into()));
            return Ok(displaced);
        }
    }
    bindings.remove(id);
    Ok(displaced)
}

pub fn same_keys(a: &str, b: &str, platform: Platform) -> bool {
    parse_accelerator(a, platform).is_some_and(|a| parse_accelerator(b, platform) == Some(a))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Key {
    Ascii(u8),
    Named(&'static str),
    Function(u8),
}

const CONTROL: u8 = 1 << 0;
const ALT: u8 = 1 << 1;
const SHIFT: u8 = 1 << 2;
const COMMAND: u8 = 1 << 3;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Accelerator {
    modifiers: u8,
    key: Key,
}

/// Matches ParseFlechaAccelerator in Chromium patch 0016.
fn parse_accelerator(text: &str, platform: Platform) -> Option<Accelerator> {
    let mut parts: Vec<_> = text
        .split('+')
        .map(|part| part.trim_matches([' ', '\t', '\n', '\x0b', '\x0c', '\r']))
        .collect();
    let tail = parts.pop()?;
    let tail = if tail.is_empty() && parts.last() == Some(&"") {
        parts.pop();
        "Plus"
    } else {
        tail
    };
    let key = key_code(tail)?;
    let mut modifiers = 0;
    for token in parts {
        modifiers |= match token.to_ascii_lowercase().as_str() {
            "cmdorctrl" | "commandorcontrol" if platform == Platform::Mac => COMMAND,
            "cmdorctrl" | "commandorcontrol" | "ctrl" | "control" => CONTROL,
            "alt" | "option" => ALT,
            "shift" => SHIFT,
            "cmd" | "command" | "super" | "meta" => COMMAND,
            _ => return None,
        };
    }
    Some(Accelerator { modifiers, key })
}

fn key_code(text: &str) -> Option<Key> {
    if let [byte] = text.as_bytes() {
        return (byte.is_ascii_alphanumeric() || b"[]=-,./;'`\\".contains(byte))
            .then(|| Key::Ascii(byte.to_ascii_uppercase()));
    }
    // VKEY_OEM_PLUS handles both the Plus spelling and the '=' key.
    let name = match text.to_ascii_lowercase().as_str() {
        "plus" => return Some(Key::Ascii(b'=')),
        "tab" => "Tab",
        "space" => "Space",
        "enter" | "return" => "Enter",
        "escape" | "esc" => "Escape",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "up" => "Up",
        "down" => "Down",
        "left" => "Left",
        "right" => "Right",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        _ => {
            let f: u8 = text.strip_prefix(['F', 'f'])?.parse().ok()?;
            return (1..=24).contains(&f).then_some(Key::Function(f));
        }
    };
    Some(Key::Named(name))
}

pub fn display_keys(keys: Option<&str>, platform: Platform) -> String {
    let Some(accelerator) = keys.and_then(|keys| parse_accelerator(keys, platform)) else {
        return String::new();
    };
    let Accelerator { modifiers, key } = accelerator;
    let mask = [CONTROL, ALT, SHIFT, COMMAND];
    let key = match key {
        Key::Ascii(byte) => char::from(byte).to_string(),
        Key::Named(name) => name.into(),
        Key::Function(n) => format!("F{n}"),
    };
    if platform == Platform::Mac {
        let mut label: String = mask
            .into_iter()
            .zip(['⌃', '⌥', '⇧', '⌘'])
            .filter_map(|(bit, symbol)| (modifiers & bit != 0).then_some(symbol))
            .collect();
        let display = match key.as_str() {
            "Left" => "←",
            "Right" => "→",
            "Up" => "↑",
            "Down" => "↓",
            "Enter" => "↩",
            "Tab" => "⇥",
            "Escape" => "Esc",
            "Backspace" => "⌫",
            "Delete" => "⌦",
            "-" => "−",
            _ => &key,
        };
        label.push_str(display);
        label
    } else {
        let mut label: Vec<_> = mask
            .into_iter()
            .zip(["Ctrl", "Alt", "Shift", "Super"])
            .filter_map(|(bit, symbol)| (modifiers & bit != 0).then_some(symbol))
            .collect();
        label.push(&key);
        label.join("+")
    }
}

pub fn space_shortcut_label(platform: Platform) -> String {
    let modifier = if platform == Platform::Mac {
        "Ctrl"
    } else {
        "Ctrl+Alt"
    };
    format!(
        "{} … {}",
        display_keys(Some(&format!("{modifier}+1")), platform),
        display_keys(Some(&format!("{modifier}+9")), platform)
    )
}
