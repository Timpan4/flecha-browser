use std::collections::{HashMap, HashSet};
use std::path::Path;

use cap_std::fs::Dir;
use flecha_core::ArchivedTab;
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::Value;

use crate::bookmarks::BookmarkTree;
use crate::input::{
    JsonRead, arc_sidebar_json, json, open_dir, open_root, text_or, truthy, web_scheme,
};
use crate::plan::tabs_in;
use crate::{Bookmark, ReadError, SavedTab};

const APPLE_EPOCH_SECONDS: f64 = 978_307_200.0;

#[derive(Serialize)]
pub struct ArcImport {
    pub spaces: Vec<ArcSpace>,
    pub favorites: Vec<SavedTab>,
    pub archive: Vec<ArchivedTab>,
    pub skipped: IndexMap<String, usize>,
}

#[derive(Serialize)]
pub struct ArcSpace {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<ArcProfile>,
    pub pinned: Vec<Bookmark>,
    pub today: Vec<SavedTab>,
    pub splits: Vec<Vec<String>>,
}

#[derive(Serialize)]
pub struct ArcProfile {
    pub dir: String,
    pub name: String,
}

pub fn read_arc(root: &Path) -> Result<(ArcImport, Vec<String>), ReadError> {
    read_arc_dir(&open_root(root)?)
}

pub(crate) fn read_arc_dir(root: &Dir) -> Result<(ArcImport, Vec<String>), ReadError> {
    let mut warnings = Vec::new();
    let sidebar = arc_sidebar_json(root)?.into_value("Arc’s sidebar", &mut warnings);
    let mut optional = |result: Result<JsonRead, ReadError>, label| match result {
        Ok(document) => document.into_value(label, &mut warnings),
        Err(ReadError::Missing) => Value::Null,
        Err(error) => {
            warnings.push(format!("Couldn’t read {label}: {error}"));
            Value::Null
        }
    };
    let profiles = optional(
        open_dir(root, Path::new("User Data")).and_then(|dir| json(&dir, "Local State")),
        "Arc’s profile names",
    );
    let archive = optional(
        json(root, "StorableArchiveItems.json"),
        "Arc’s archived tabs",
    );
    let (mut arc, bad_items, shared_items) =
        parse_sidebar(&sidebar, &profiles["profile"]["info_cache"])?;
    let (archive, bad_archive) = parse_archive(&archive);
    arc.archive = archive;
    if bad_items > 0 {
        warnings.push(format!(
            "Skipped or repaired {bad_items} malformed Arc sidebar records."
        ));
    }
    if shared_items > 0 {
        warnings.push(format!(
            "Kept {shared_items} shared Arc sidebar references at their first location."
        ));
    }
    if bad_archive > 0 {
        warnings.push(format!(
            "Skipped {bad_archive} malformed Arc archive records."
        ));
    }
    Ok((arc, warnings))
}

fn array(value: &Value) -> &[Value] {
    value.as_array().map_or(&[], Vec::as_slice)
}

fn parse_sidebar(sidebar: &Value, names: &Value) -> Result<(ArcImport, usize, usize), ReadError> {
    let main = array(&sidebar["sidebar"]["containers"])
        .iter()
        .find(|c| c["spaces"].is_array() && c["items"].is_array())
        .ok_or(ReadError::InvalidSidebar)?;
    let mut walker = Walker::default();
    for entry in array(&main["items"]) {
        if let Some(id) = entry["id"].as_str() {
            walker.items.insert(id, entry);
        } else if !entry.is_string() {
            walker.bad_records += 1;
        }
    }

    let mut spaces = Vec::new();
    for raw in array(&main["spaces"]) {
        if !raw.is_object() {
            walker.bad_records += usize::from(!raw.is_string());
            continue;
        }
        let ids = if raw["containerIDs"].is_null() {
            &raw["newContainerIDs"]
        } else {
            &raw["containerIDs"]
        };
        walker.bad_records += usize::from(!ids.is_null() && !ids.is_array());
        let container = |key: &str| {
            let ids = array(ids);
            ids.iter()
                .position(|marker| marker.as_str() == Some(key) || marker.get(key).is_some())
                .and_then(|at| ids.get(at + 1)?.as_str())
        };
        let mut splits = Vec::new();
        let pinned = walker.container(container("pinned"), &mut splits);
        let today = tabs_in(&walker.container(container("unpinned"), &mut splits))
            .cloned()
            .collect();
        let dir = raw["profile"]["custom"]["_0"]["directoryBasename"]
            .as_str()
            .filter(|dir| !dir.is_empty());
        let info = &raw["customInfo"];
        spaces.push(ArcSpace {
            id: raw["id"].as_str().map(str::to_owned),
            name: text_or(&raw["title"], "Space").into(),
            emoji: info["iconType"]["emoji_v2"].as_str().map(str::to_owned),
            color: theme_color(&info["windowTheme"]["primaryColorPalette"]["midTone"]),
            profile: dir.map(|dir| ArcProfile {
                dir: dir.into(),
                name: text_or(&names[dir]["name"], dir).into(),
            }),
            pinned,
            today,
            splits,
        });
    }

    let favorites_ids = &main["topAppsContainerIDs"];
    walker.bad_records += usize::from(!favorites_ids.is_null() && !favorites_ids.is_array());
    let mut unique = HashSet::new();
    let mut favorites = Vec::new();
    for id in array(favorites_ids).iter().filter_map(Value::as_str) {
        for tab in tabs_in(&walker.container(Some(id), &mut Vec::new())) {
            if unique.insert(tab.url.clone()) {
                favorites.push(tab.clone());
            }
        }
    }
    Ok((
        ArcImport {
            spaces,
            favorites,
            archive: Vec::new(),
            skipped: walker.skipped,
        },
        walker.bad_records,
        walker.shared_items,
    ))
}

#[derive(Default)]
struct Walker<'a> {
    items: HashMap<&'a str, &'a Value>,
    owners: HashMap<&'a str, usize>,
    walk: usize,
    skipped: IndexMap<String, usize>,
    bad_records: usize,
    shared_items: usize,
}

enum Claim {
    First,
    SameWalk,
    Shared,
}

#[derive(Default)]
struct DirectTabs<'a> {
    urls: Vec<&'a str>,
    claimed_end: usize,
}

enum Action<'a> {
    Children(std::slice::Iter<'a, Value>),
    EndFolder,
    EndSplit { at: usize, lost_before: usize },
    SkipKind(&'static str),
}

impl<'a> Walker<'a> {
    fn container(&mut self, id: Option<&'a str>, splits: &mut Vec<Vec<String>>) -> Vec<Bookmark> {
        let Some(id) = id else { return Vec::new() };
        let Some(item) = self.items.get(id).copied() else {
            return Vec::new();
        };
        self.walk += 1;
        if !matches!(self.claim(id), Claim::First) {
            return Vec::new();
        }

        let mut tree = BookmarkTree::default();
        let mut direct = vec![DirectTabs::default()];
        let mut lost = 0;
        let mut stack = vec![self.children(&item["childrenIds"])];
        while let Some(action) = stack.pop() {
            match action {
                Action::Children(mut ids) => {
                    let Some(id) = ids.next() else { continue };
                    stack.push(Action::Children(ids));
                    let Some(id) = id.as_str() else {
                        self.bad_records += 1;
                        continue;
                    };
                    let Some(item) = self.items.get(id).copied() else {
                        continue;
                    };
                    match self.claim(id) {
                        Claim::First => {}
                        Claim::SameWalk => continue,
                        Claim::Shared => {
                            lost += 1;
                            continue;
                        }
                    }
                    let data = &item["data"];
                    if truthy(&data["list"]) {
                        tree.open(text_or(&item["title"], "Untitled folder").into());
                        direct.push(DirectTabs::default());
                        stack.push(Action::EndFolder);
                    } else if truthy(&data["tab"]) {
                        match tab(item) {
                            Some((url, title)) => {
                                direct.last_mut().expect("current list").urls.push(url);
                                tree.push(Bookmark::Tab(SavedTab {
                                    url: url.into(),
                                    title: title.into(),
                                }));
                            }
                            None => {
                                lost += 1;
                                if data["tab"]["savedURL"].as_str().is_none_or(str::is_empty) {
                                    self.skip("tab without address");
                                } else {
                                    self.skip("non-web tab");
                                }
                            }
                        }
                    } else if truthy(&data["splitView"]) {
                        stack.push(Action::EndSplit {
                            at: direct.last().expect("current list").urls.len(),
                            lost_before: lost,
                        });
                    } else {
                        // Item keys are untrusted too. Only published Arc kinds reach diagnostics.
                        let first = data
                            .as_object()
                            .and_then(|data| data.keys().next())
                            .map(String::as_str);
                        let kind = match first {
                            Some("easel") => "easel",
                            Some("welcomeToArc") => "welcomeToArc",
                            Some("boost") => "boost",
                            Some("itemContainer") => "itemContainer",
                            _ => "unknown",
                        };
                        stack.push(Action::SkipKind(kind));
                    }
                    stack.push(self.children(&item["childrenIds"]));
                }
                Action::EndFolder => {
                    direct.pop();
                    tree.close(true);
                }
                Action::EndSplit { at, lost_before } => {
                    let list = direct.last_mut().expect("current list");
                    if lost > lost_before {
                        self.skip("partial split");
                    } else if list.urls.len() - at > 1 {
                        // Nested splits finish inside-out. A direct tab can belong to
                        // one retained split, so the total number of URL copies is linear.
                        if list.claimed_end > at {
                            self.skip("overlapping split");
                        } else {
                            splits.push(
                                list.urls[at..]
                                    .iter()
                                    .map(|url| (*url).to_owned())
                                    .collect(),
                            );
                            list.claimed_end = list.urls.len();
                        }
                    }
                }
                Action::SkipKind(kind) => self.skip(kind),
            }
        }
        tree.finish()
    }

    fn claim(&mut self, id: &'a str) -> Claim {
        use std::collections::hash_map::Entry;

        match self.owners.entry(id) {
            Entry::Vacant(entry) => {
                entry.insert(self.walk);
                Claim::First
            }
            Entry::Occupied(entry) if *entry.get() == self.walk => Claim::SameWalk,
            Entry::Occupied(_) => {
                self.shared_items += 1;
                Claim::Shared
            }
        }
    }

    fn children(&mut self, ids: &'a Value) -> Action<'a> {
        self.bad_records += usize::from(!ids.is_null() && !ids.is_array());
        Action::Children(array(ids).iter())
    }

    fn skip(&mut self, kind: &str) {
        *self.skipped.entry(kind.into()).or_default() += 1;
    }
}

fn tab(item: &Value) -> Option<(&str, &str)> {
    let data = &item["data"]["tab"];
    let url = data["savedURL"].as_str().filter(|url| web_scheme(url))?;
    let title = text_or(&item["title"], text_or(&data["savedTitle"], url));
    Some((url, title))
}

fn parse_archive(archive: &Value) -> (Vec<ArchivedTab>, usize) {
    let mut output = Vec::new();
    let mut bad = 0;
    for entry in array(&archive["items"]) {
        if entry.is_string() {
            continue;
        }
        let Some(seconds) = entry["archivedAt"].as_f64() else {
            bad += 1;
            continue;
        };
        let time = (seconds + APPLE_EPOCH_SECONDS) * 1000.0;
        let floor = time.floor();
        let rounded = if time - floor >= 0.5 {
            floor + 1.0
        } else {
            floor
        };
        if !(i64::MIN as f64..-(i64::MIN as f64)).contains(&rounded) {
            bad += 1;
            continue;
        }
        let item = &entry["sidebarItem"];
        if let Some((url, title)) = tab(item) {
            output.push(ArchivedTab {
                url: url.into(),
                title: title.into(),
                closed_at: rounded as i64,
            });
        } else if !item["data"]["tab"]["savedURL"].is_string() {
            bad += 1;
        }
    }
    output.sort_by_key(|tab| std::cmp::Reverse(tab.closed_at));
    (output, bad)
}

fn theme_color(color: &Value) -> Option<String> {
    let channel = |name: &str| {
        color[name]
            .as_f64()
            .map(|value| (value.clamp(0.0, 1.0) * 255.0).round() as u8)
    };
    let (red, green, blue) = (channel("red")?, channel("green")?, channel("blue")?);
    Some(format!("#{red:02x}{green:02x}{blue:02x}"))
}
