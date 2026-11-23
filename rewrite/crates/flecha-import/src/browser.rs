use std::path::{Path, PathBuf};

use cap_std::fs::Dir;
use serde::Serialize;
use serde_json::Value;

use crate::arc::read_arc_dir;
use crate::bookmarks::{BookmarkTree, Tidy};
use crate::input::{dirs, file_exists, js_string, json, open_dir, open_root, truthy};
use crate::safari;
use crate::sqlite::{Database, Snapshot};
use crate::{Bookmark, ImportPlan, ReadError, SavedTab};

const FULL_DISK_ACCESS: &str = "macOS only lets apps read Safari’s data with Full Disk Access. Turn it on for Flecha in System Settings › Privacy & Security › Full Disk Access, then import again.";

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Browser {
    Chrome,
    Brave,
    Edge,
    Vivaldi,
    Opera,
    OperaGx,
    Helium,
    Chromium,
    Arc,
    Firefox,
    Safari,
}

impl Browser {
    pub fn id(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Brave => "brave",
            Self::Edge => "edge",
            Self::Vivaldi => "vivaldi",
            Self::Opera => "opera",
            Self::OperaGx => "opera-gx",
            Self::Helium => "helium",
            Self::Chromium => "chromium",
            Self::Arc => "arc",
            Self::Firefox => "firefox",
            Self::Safari => "safari",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Chrome => "Chrome",
            Self::Brave => "Brave",
            Self::Edge => "Edge",
            Self::Vivaldi => "Vivaldi",
            Self::Opera => "Opera",
            Self::OperaGx => "Opera GX",
            Self::Helium => "Helium",
            Self::Chromium => "Chromium",
            Self::Arc => "Arc",
            Self::Firefox => "Firefox",
            Self::Safari => "Safari",
        }
    }

    fn database(self) -> Database {
        match self {
            Self::Firefox => Database::Firefox,
            Self::Safari => Database::Safari,
            _ => Database::Chromium,
        }
    }
}

#[derive(Clone, Copy)]
pub enum Platform {
    Linux,
    MacOs,
    Windows,
}

/// Paths supplied by the application. Discovery never reads process environment itself.
pub struct BrowserLocations {
    pub platform: Platform,
    pub home: PathBuf,
    pub xdg_config_home: Option<PathBuf>,
    pub chrome_config_home: Option<PathBuf>,
    pub helium_config_home: Option<PathBuf>,
    pub local_app_data: Option<PathBuf>,
    pub roaming_app_data: Option<PathBuf>,
}

impl BrowserLocations {
    pub fn new(platform: Platform, home: impl Into<PathBuf>) -> Self {
        Self {
            platform,
            home: home.into(),
            xdg_config_home: None,
            chrome_config_home: None,
            helium_config_home: None,
            local_app_data: None,
            roaming_app_data: None,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedBrowser {
    pub id: Browser,
    pub name: &'static str,
    pub profiles: usize,
    pub has_bookmarks: bool,
    pub has_history: bool,
}

struct Location<'a> {
    anchor: &'a Path,
    relative: PathBuf,
}

impl<'a> Location<'a> {
    fn child(&self, path: impl AsRef<Path>) -> Self {
        Self {
            anchor: self.anchor,
            relative: self.relative.join(path),
        }
    }

    fn open(&self) -> Result<Dir, ReadError> {
        open_dir(&open_root(self.anchor)?, &self.relative)
    }
}

const CHROMIUM: [(Browser, &str, &str, &str); 8] = [
    (
        Browser::Chrome,
        "Google/Chrome",
        "google-chrome",
        "Google/Chrome/User Data",
    ),
    (
        Browser::Brave,
        "BraveSoftware/Brave-Browser",
        "BraveSoftware/Brave-Browser",
        "BraveSoftware/Brave-Browser/User Data",
    ),
    (
        Browser::Edge,
        "Microsoft Edge",
        "microsoft-edge",
        "Microsoft/Edge/User Data",
    ),
    (Browser::Vivaldi, "Vivaldi", "vivaldi", "Vivaldi/User Data"),
    (
        Browser::Opera,
        "com.operasoftware.Opera",
        "opera",
        "Opera Software/Opera Stable",
    ),
    (
        Browser::OperaGx,
        "com.operasoftware.OperaGX",
        "",
        "Opera Software/Opera GX Stable",
    ),
    (
        Browser::Helium,
        "net.imput.helium",
        "net.imput.helium",
        "imput/Helium/User Data",
    ),
    (
        Browser::Chromium,
        "Chromium",
        "chromium",
        "Chromium/User Data",
    ),
];

fn location<'a>(anchor: &'a Path, relative: &str) -> Location<'a> {
    Location {
        anchor,
        relative: relative.into(),
    }
}

fn locations(config: &BrowserLocations) -> Vec<(Browser, Location<'_>)> {
    let home = &config.home;
    match config.platform {
        Platform::Linux => {
            let config_dir = config
                .xdg_config_home
                .as_deref()
                .filter(|p| p.is_absolute())
                .map_or_else(|| location(home, ".config"), |p| location(p, ""));
            let chromium = CHROMIUM
                .iter()
                .filter(|(_, _, linux, _)| !linux.is_empty())
                .map(|&(browser, _, linux, _)| {
                    let overrides = match browser {
                        Browser::Chrome | Browser::Chromium => config.chrome_config_home.as_deref(),
                        Browser::Helium => config.helium_config_home.as_deref(),
                        _ => None,
                    };
                    let root = overrides
                        .filter(|p| p.is_absolute())
                        .map_or_else(|| config_dir.child(linux), |p| location(p, linux));
                    (browser, root)
                });
            chromium
                .chain([(Browser::Firefox, location(home, ".mozilla/firefox"))])
                .collect()
        }
        Platform::MacOs => {
            let support = location(home, "Library/Application Support");
            CHROMIUM
                .iter()
                .map(|&(browser, mac, _, _)| (browser, support.child(mac)))
                .chain([
                    (Browser::Arc, support.child("Arc")),
                    (Browser::Firefox, support.child("Firefox/Profiles")),
                    (Browser::Safari, location(home, "Library/Safari")),
                ])
                .collect()
        }
        Platform::Windows => {
            let local = config
                .local_app_data
                .as_deref()
                .map_or_else(|| location(home, "AppData/Local"), |p| location(p, ""));
            let roaming = config
                .roaming_app_data
                .as_deref()
                .map_or_else(|| location(home, "AppData/Roaming"), |p| location(p, ""));
            CHROMIUM
                .iter()
                .map(|&(browser, _, _, windows)| {
                    let base = if matches!(browser, Browser::Opera | Browser::OperaGx) {
                        &roaming
                    } else {
                        &local
                    };
                    (browser, base.child(windows))
                })
                .chain([
                    (Browser::Arc, arc_windows_dir(&local)),
                    (Browser::Firefox, roaming.child("Mozilla/Firefox/Profiles")),
                ])
                .collect()
        }
    }
}

fn arc_windows_dir<'a>(local: &Location<'a>) -> Location<'a> {
    let packages = local.child("Packages");
    let mut names = packages
        .open()
        .and_then(|dir| dirs(&dir))
        .unwrap_or_default();
    names.sort_unstable();
    let candidates = names
        .iter()
        .filter(|name| name.to_string_lossy().starts_with("TheBrowserCompany.Arc_"))
        .map(|name| packages.child(name).child("LocalCache/Local/Arc"));
    let first = candidates.clone().next();
    candidates
        .clone()
        .find(|candidate| {
            candidate.open().is_ok_and(|dir| {
                file_exists(&dir, "StorableSidebar.json").unwrap_or(false)
                    || open_dir(&dir, Path::new("User Data"))
                        .and_then(|data| dirs(&data))
                        .is_ok_and(|dirs| !dirs.is_empty())
            })
        })
        .or(first)
        .unwrap_or_else(|| packages.child("TheBrowserCompany.Arc/LocalCache/Local/Arc"))
}

struct Profile {
    root: Dir,
    name: String,
    bookmarks: Vec<&'static str>,
    history: Option<&'static str>,
}

fn profiles(
    browser: Browser,
    root: &Dir,
    read_names: bool,
    warnings: &mut Vec<String>,
) -> Vec<Profile> {
    if browser == Browser::Safari {
        return root
            .try_clone()
            .ok()
            .and_then(|root| {
                profile(
                    root,
                    "Safari".into(),
                    &["Bookmarks.plist"],
                    "History.db",
                    browser,
                    warnings,
                )
            })
            .into_iter()
            .collect();
    }
    let data;
    let root = if browser == Browser::Arc {
        data = match open_dir(root, Path::new("User Data")) {
            Ok(data) => data,
            Err(ReadError::Missing) => return Vec::new(),
            Err(error) => {
                warn(warnings, browser, "Arc profiles", error);
                return Vec::new();
            }
        };
        &data
    } else {
        root
    };
    let mut names = match dirs(root) {
        Ok(names) => names,
        Err(error) => {
            warn(warnings, browser, "browser profiles", error);
            Vec::new()
        }
    };
    names.sort_unstable();
    let mut all = Vec::new();
    if browser == Browser::Firefox {
        for dir in &names {
            match open_dir(root, Path::new(dir)) {
                Ok(dir_root) => {
                    let name = dir.to_string_lossy();
                    let short = name
                        .split_once('.')
                        .filter(|(prefix, _)| !prefix.is_empty())
                        .map_or(name.as_ref(), |(_, name)| name);
                    all.extend(profile(
                        dir_root,
                        short.into(),
                        &["places.sqlite"],
                        "places.sqlite",
                        browser,
                        warnings,
                    ));
                }
                Err(error) => warn(warnings, browser, "Firefox profiles", error),
            }
        }
    } else {
        let cache = match read_names.then(|| json(root, "Local State")) {
            Some(Ok(state)) => state.into_value("browser profile names", warnings),
            None | Some(Err(ReadError::Missing)) => Value::Null,
            Some(Err(error)) => {
                warn(warnings, browser, "browser profile names", error);
                Value::Null
            }
        };
        let names_cache = &cache["profile"]["info_cache"];
        let mut add = |dir: Result<Dir, ReadError>, key: &str| match dir {
            Ok(dir) => {
                let name = &names_cache[key]["name"];
                let name = if name.is_null() {
                    String::new()
                } else {
                    js_string(name)
                };
                let name = if name.is_empty() { key.into() } else { name };
                all.extend(profile(
                    dir,
                    name,
                    &["Bookmarks", "AccountBookmarks"],
                    "History",
                    browser,
                    warnings,
                ));
            }
            Err(error) => warn(warnings, browser, "browser profiles", error),
        };
        add(root.try_clone().map_err(crate::input::io_error), "Default");
        for dir in &names {
            let Some(name) = dir.to_str() else { continue };
            if name == "Default"
                || name
                    .strip_prefix("Profile ")
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|ch| ch.is_ascii_digit()))
            {
                add(open_dir(root, Path::new(dir)), name);
            }
        }
    }
    all
}

fn profile(
    root: Dir,
    name: String,
    bookmarks: &[&'static str],
    history: &'static str,
    browser: Browser,
    warnings: &mut Vec<String>,
) -> Option<Profile> {
    let mut available = |name: &str| match file_exists(&root, name) {
        Ok(found) => found,
        Err(error) => {
            if error == ReadError::PermissionDenied && browser == Browser::Safari {
                true
            } else {
                warn(warnings, browser, name, error);
                false
            }
        }
    };
    let bookmarks: Vec<_> = bookmarks
        .iter()
        .copied()
        .filter(|name| available(name))
        .collect();
    let history = available(history).then_some(history);
    (!bookmarks.is_empty() || history.is_some()).then_some(Profile {
        root,
        name,
        bookmarks,
        history,
    })
}

pub fn detect_browsers(config: &BrowserLocations) -> Vec<DetectedBrowser> {
    locations(config)
        .into_iter()
        .filter_map(|(id, source)| {
            let root = match source.open() {
                Ok(root) => root,
                Err(ReadError::PermissionDenied) if id == Browser::Safari => {
                    return Some(DetectedBrowser {
                        id,
                        name: id.name(),
                        profiles: 1,
                        has_bookmarks: true,
                        has_history: true,
                    });
                }
                Err(_) => return None,
            };
            let profiles = profiles(id, &root, false, &mut Vec::new());
            let sidebar =
                id == Browser::Arc && file_exists(&root, "StorableSidebar.json").unwrap_or(false);
            (!profiles.is_empty() || sidebar).then(|| DetectedBrowser {
                id,
                name: id.name(),
                profiles: profiles.len().max(1),
                has_bookmarks: sidebar || profiles.iter().any(|p| !p.bookmarks.is_empty()),
                has_history: profiles.iter().any(|p| p.history.is_some()),
            })
        })
        .collect()
}

pub fn read_browser(browser: Browser, config: &BrowserLocations) -> ImportPlan {
    let Some((_, source)) = locations(config).into_iter().find(|(id, _)| *id == browser) else {
        let mut plan = ImportPlan::new(browser.id());
        plan.warnings.push(format!(
            "No supported data from “{}” was found on this computer.",
            browser.id()
        ));
        return plan;
    };
    let mut plan = ImportPlan::new(browser.name());
    let root = match source.open() {
        Ok(root) => root,
        Err(error) => {
            if browser == Browser::Arc || error != ReadError::Missing {
                let what = if browser == Browser::Arc {
                    "Arc’s sidebar"
                } else {
                    browser.name()
                };
                warn(&mut plan.warnings, browser, what, error);
            }
            return plan;
        }
    };
    let profiles = profiles(browser, &root, browser != Browser::Arc, &mut plan.warnings);
    if browser == Browser::Arc {
        match read_arc_dir(&root) {
            Ok((arc, warnings)) => {
                plan.arc = Some(arc);
                plan.warnings.extend(warnings);
            }
            Err(error) => warn(&mut plan.warnings, browser, "Arc’s sidebar", error),
        }
    }
    let mut per_profile = Vec::new();
    let mut tidy = Tidy::default();
    let mut malformed_bookmarks = 0;
    let mut malformed_history = 0;
    let kind = browser.database();
    for profile in profiles {
        let snapshot = profile
            .history
            .map(|name| Snapshot::open(&profile.root, name, kind));
        if let Some(Ok(copy)) = &snapshot
            && let Some(error) = copy.wal_warning
        {
            warn(
                &mut plan.warnings,
                browser,
                "browser history journal",
                error,
            );
        }
        let label = browser.name();
        if !profile.bookmarks.is_empty() && browser != Browser::Arc {
            let (nodes, skipped) = match browser {
                Browser::Firefox => match &snapshot {
                    Some(Ok(copy)) => copy.firefox_bookmarks(),
                    Some(Err(error)) => Err(*error),
                    None => Err(ReadError::Missing),
                }
                .unwrap_or_else(|error| {
                    warn(
                        &mut plan.warnings,
                        browser,
                        &format!("bookmarks from {label}"),
                        error,
                    );
                    (Vec::new(), 0)
                }),
                Browser::Safari => safari::bookmarks(&profile.root).unwrap_or_else(|error| {
                    warn(
                        &mut plan.warnings,
                        browser,
                        &format!("bookmarks from {label}"),
                        error,
                    );
                    (Vec::new(), 0)
                }),
                _ => chromium_bookmarks(&profile, browser, &mut plan.warnings),
            };
            malformed_bookmarks += skipped;
            let nodes = tidy.run(&nodes);
            if !nodes.is_empty() {
                per_profile.push((profile.name, nodes));
            }
        }
        if let Some(snapshot) = snapshot {
            match snapshot.and_then(|copy| copy.history(kind)) {
                Ok((history, skipped)) => {
                    malformed_history += skipped;
                    plan.history.extend(history);
                }
                Err(error) => warn(
                    &mut plan.warnings,
                    browser,
                    &format!("history from {label}"),
                    error,
                ),
            }
        }
    }
    let bookmarks = match per_profile.len() {
        0 => Vec::new(),
        1 => per_profile.pop().expect("one profile with bookmarks").1,
        _ => per_profile
            .into_iter()
            .map(|(name, children)| Bookmark::Folder { name, children })
            .collect(),
    };
    tidy.finish(&mut plan, bookmarks);
    if malformed_bookmarks > 0 {
        plan.warnings.push(format!(
            "Skipped {malformed_bookmarks} malformed bookmark records."
        ));
    }
    if malformed_history > 0 {
        plan.warnings.push(format!(
            "Skipped {malformed_history} malformed history records."
        ));
    }
    plan
}

fn warn(warnings: &mut Vec<String>, browser: Browser, what: &str, error: ReadError) {
    let warning = if browser == Browser::Safari && error == ReadError::PermissionDenied {
        FULL_DISK_ACCESS.into()
    } else {
        format!("Couldn’t read {what}: {error}")
    };
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

fn chromium_bookmarks(
    profile: &Profile,
    browser: Browser,
    warnings: &mut Vec<String>,
) -> (Vec<Bookmark>, usize) {
    let mut roots: [Vec<Bookmark>; 3] = Default::default();
    let mut malformed = 0;
    for file in &profile.bookmarks {
        match json(&profile.root, file) {
            Ok(document) => {
                let document = document.into_value(file, warnings);
                for (nodes, root) in roots.iter_mut().zip(["bookmark_bar", "other", "synced"]) {
                    nodes.extend(chromium_children(&document["roots"][root], &mut malformed));
                }
            }
            Err(error) => warn(
                warnings,
                browser,
                &format!("bookmarks from {}", browser.name()),
                error,
            ),
        }
    }
    let [mut bar, other, mobile] = roots;
    bar.extend([
        Bookmark::Folder {
            name: "Other Bookmarks".into(),
            children: other,
        },
        Bookmark::Folder {
            name: "Mobile Bookmarks".into(),
            children: mobile,
        },
    ]);
    (bar, malformed)
}

fn chromium_children(root: &Value, malformed: &mut usize) -> Vec<Bookmark> {
    fn children<'a>(node: &'a Value, malformed: &mut usize) -> std::slice::Iter<'a, Value> {
        let children = &node["children"];
        if let Some(children) = children.as_array() {
            children.iter()
        } else {
            *malformed += usize::from(!children.is_null());
            [].iter()
        }
    }

    let mut tree = BookmarkTree::default();
    let mut stack = vec![children(root, malformed)];
    while let Some(items) = stack.last_mut() {
        let Some(node) = items.next() else {
            stack.pop();
            if !stack.is_empty() {
                tree.close(true);
            }
            continue;
        };
        match node["type"].as_str() {
            Some("url") if node["url"].is_string() => {
                let url = node["url"].as_str().expect("checked bookmark URL");
                let title = if truthy(&node["name"]) {
                    js_string(&node["name"])
                } else {
                    url.into()
                };
                tree.push(Bookmark::Tab(SavedTab {
                    url: url.into(),
                    title,
                }));
            }
            Some("folder") => {
                let name = if truthy(&node["name"]) {
                    js_string(&node["name"])
                } else {
                    "Untitled folder".into()
                };
                tree.open(name);
                stack.push(children(node, malformed));
            }
            _ => *malformed += 1,
        }
    }
    tree.finish()
}
