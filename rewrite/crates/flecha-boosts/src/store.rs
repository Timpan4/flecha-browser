//! The saved Boosts and what they do to pages, from the shell's `features/boosts.ts`.

use flecha_core::input_text::trim_input;
use indexmap::IndexMap;
use serde::Serialize;

use crate::compile::{CompiledBoost, console_error, error_message, once_per_document};
use crate::matching::{applies, boost_host_for, pattern_host, user_script_matches};
use crate::name::{BoostKind, BoostName};

/// One saved Boost.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Entry {
    pub(crate) code: String,
    /// A script's last build. The shell keeps no build for styles, but reports one it finds.
    pub(crate) compiled: Option<CompiledBoost>,
}

/// Every saved Boost, in the order it was first saved.
///
/// Boosts apply in that order, as the shell's `Object.keys` order did. [`Boosts::list`] sorts.
/// Incognito windows apply Boosts but must not write them: the app calls the writers only
/// for a regular profile, as `boosts.ts` checks `incognito`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Boosts {
    pub(crate) entries: IndexMap<BoostName, Entry>,
}

/// `BoostFile`: a Boost as the Boosts page shows it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BoostFile {
    pub name: BoostName,
    pub host: String,
    pub kind: BoostKind,
    pub code: String,
    /// For scripts: why the last save didn't compile, if it didn't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What a write changed, and which open pages have to follow it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Effect {
    /// Nothing changed, so nothing needs saving.
    Unchanged,
    /// Saved. Open pages keep what they have: a removed script stays until the page reloads,
    /// and a new template or a save without reload changes no page.
    Saved,
    /// Saved. Replace the styles of every open page this host's Boosts apply to with
    /// [`Boosts::styles_for`] at once, without a reload.
    Restyle { host: String },
    /// Saved. Reload every open page this host's Boosts apply to.
    Reload { host: String },
}

impl Effect {
    pub fn changed(&self) -> bool {
        !matches!(self, Self::Unchanged)
    }

    /// Whether the open page at `url` has to be restyled or reloaded: [`applies`] for the host.
    ///
    /// The shell reloaded every tab `boostMatches` took, including `chrome://`, extension and
    /// `file:` tabs, and restyled all but the `file:` ones, which Chromium kept it out of. The
    /// rewrite leaves non-web pages alone on purpose.
    pub fn affects(&self, url: &str) -> bool {
        match self {
            Self::Restyle { host } | Self::Reload { host } => applies(url, host),
            Self::Unchanged | Self::Saved => false,
        }
    }
}

/// The frame a document loads in. Boosts apply to a tab's main frame only.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Frame {
    Main,
    Sub,
}

/// When an injection runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunAt {
    /// When the navigation commits, before the document parses (`webNavigation.onCommitted`).
    DocumentStart,
    /// Once the DOM is ready (`document_end`, `webNavigation.onDOMContentLoaded`).
    DocumentEnd,
}

/// A style Boost to insert as a user style sheet (`insertCSS` with `origin: 'USER'`).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StyleInjection {
    pub name: BoostName,
    pub css: String,
}

impl StyleInjection {
    pub const RUN_AT: RunAt = RunAt::DocumentStart;
}

/// A script Boost to run in the page's own world (`world: 'MAIN'`), as a classic script.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScriptInjection {
    /// Run the compiled Boost, wrapped to run once per document.
    Run { name: BoostName, source: String },
    /// The Boost didn't compile: log this line to the page's console.
    ReportError { name: BoostName, message: String },
}

impl ScriptInjection {
    pub const RUN_AT: RunAt = RunAt::DocumentEnd;

    pub fn name(&self) -> &BoostName {
        match self {
            Self::Run { name, .. } | Self::ReportError { name, .. } => name,
        }
    }

    /// The script to run in the page.
    pub fn source(&self) -> String {
        match self {
            Self::Run { source, .. } => source.clone(),
            Self::ReportError { message, .. } => console_error(message),
        }
    }
}

impl Boosts {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// `list`: every Boost, sorted by name in UTF-16 order as JavaScript's `sort` does.
    pub fn list(&self) -> Vec<BoostFile> {
        let mut names: Vec<_> = self.entries.keys().collect();
        names.sort_by(|a, b| a.as_str().encode_utf16().cmp(b.as_str().encode_utf16()));
        names.into_iter().map(|name| self.file(name)).collect()
    }

    /// `get`
    pub fn get(&self, name: &str) -> Option<BoostFile> {
        let name = BoostName::parse(name)?;
        self.entries.contains_key(&name).then(|| self.file(&name))
    }

    /// `boostedHosts`: hosts with at least one Boost, for the sidebar's marker.
    pub fn boosted_hosts(&self) -> Vec<&str> {
        let mut hosts: Vec<&str> = Vec::new();
        for name in self.entries.keys() {
            if !hosts.contains(&name.host()) {
                hosts.push(name.host());
            }
        }
        hosts
    }

    /// `fileFor`: the Boost for a page's site, created from the template the first time.
    pub fn file_for(&mut self, url: Option<&str>, kind: BoostKind) -> Option<(BoostName, Effect)> {
        self.create(&boost_host_for(url)?, kind)
    }

    /// `create`: a Boost for a host (`github.com`, or `all`), created from the template if new.
    pub fn create(&mut self, host: &str, kind: BoostKind) -> Option<(BoostName, Effect)> {
        let host = trim_input(host).to_lowercase();
        let host = host.strip_prefix("www.").unwrap_or(&host);
        if !is_new_boost_host(host) {
            return None;
        }
        let name = BoostName::for_host(host, kind)?;
        if self.entries.contains_key(&name) {
            return Some((name, Effect::Unchanged));
        }
        let (_, effect) = self.save(name.as_str(), &template(host, kind), None, false)?;
        Some((name, effect))
    }

    /// `save`: store a Boost. For a script, `build` is the editor's compile of `code`.
    ///
    /// A script's previous build is replaced, with an empty one when there's no `build`.
    /// `reload: false` keeps open pages of a script's site as they are.
    pub fn save(
        &mut self,
        name: &str,
        code: &str,
        build: Option<CompiledBoost>,
        reload: bool,
    ) -> Option<(BoostFile, Effect)> {
        let name = BoostName::parse(name).filter(|name| is_saved_host(name.host()))?;
        let kind = name.kind();
        let entry = self.entries.entry(name.clone()).or_insert(Entry {
            code: String::new(),
            compiled: None,
        });
        entry.code = code.to_owned();
        if kind == BoostKind::Script {
            entry.compiled = Some(build.unwrap_or_default());
        }
        let host = name.host().to_owned();
        let effect = match kind {
            BoostKind::Styles => Effect::Restyle { host },
            BoostKind::Script if reload => Effect::Reload { host },
            BoostKind::Script => Effect::Saved,
        };
        Some((self.file(&name), effect))
    }

    /// `remove`: unstyle pages now. A removed script stays until its page reloads.
    pub fn remove(&mut self, name: &str) -> Effect {
        let Some(name) = BoostName::parse(name) else {
            return Effect::Unchanged;
        };
        if self.entries.shift_remove(&name).is_none() {
            return Effect::Unchanged;
        }
        match name.kind() {
            BoostKind::Styles => Effect::Restyle {
                host: name.host().to_owned(),
            },
            BoostKind::Script => Effect::Saved,
        }
    }

    /// The style Boosts for a document at `url`, inserted when its navigation commits.
    pub fn styles_for(&self, frame: Frame, url: &str) -> Vec<StyleInjection> {
        self.entries
            .iter()
            .filter(|(name, _)| {
                frame == Frame::Main
                    && name.kind() == BoostKind::Styles
                    && applies(url, name.host())
            })
            .map(|(name, entry)| StyleInjection {
                name: name.clone(),
                css: entry.code.clone(),
            })
            .collect()
    }

    /// The script Boosts for a document at `url`, run once its DOM is ready.
    ///
    /// Inject each one as its own script, so no Boost's text can reach another's.
    ///
    /// The shell reached a document two ways, and a Boost ran where either matched. On
    /// `DOMContentLoaded` it executed the Boosts [`applies`] picks, and reported build errors.
    /// It also registered every compiled script with Chromium, which matched by
    /// [`user_script_matches`]. Registration was all or nothing: one pattern Chromium rejects
    /// left every script to the first path.
    pub fn scripts_for(&self, frame: Frame, url: &str) -> Vec<ScriptInjection> {
        if frame != Frame::Main {
            return Vec::new();
        }
        let registered = self.registered();
        self.entries
            .iter()
            .filter(|(name, _)| name.kind() == BoostKind::Script)
            .filter_map(|(name, entry)| {
                let compiled = entry.compiled.as_ref()?;
                let executed = applies(url, name.host());
                match (&compiled.code, &compiled.error) {
                    (Some(code), _)
                        if !code.is_empty()
                            && (executed
                                || registered && user_script_matches(url, name.host())) =>
                    {
                        Some(ScriptInjection::Run {
                            name: name.clone(),
                            source: once_per_document(name, code),
                        })
                    }
                    (Some(code), _) if !code.is_empty() => None,
                    (_, Some(error)) if executed && !error.is_empty() => {
                        Some(ScriptInjection::ReportError {
                            name: name.clone(),
                            message: error_message(name, error),
                        })
                    }
                    _ => None,
                }
            })
            .collect()
    }

    /// Whether Chromium accepted the shell's user script registration.
    fn registered(&self) -> bool {
        self.entries
            .iter()
            .filter(|(name, entry)| {
                name.kind() == BoostKind::Script
                    && entry
                        .compiled
                        .as_ref()
                        .and_then(|c| c.code.as_deref())
                        .is_some_and(|code| !code.is_empty())
            })
            .all(|(name, _)| pattern_host(name.host()).is_some())
    }

    /// Every Boost in apply order, with its stored code and build.
    pub fn iter(&self) -> impl Iterator<Item = (&BoostName, &str, Option<&CompiledBoost>)> {
        self.entries
            .iter()
            .map(|(name, entry)| (name, entry.code.as_str(), entry.compiled.as_ref()))
    }

    fn file(&self, name: &BoostName) -> BoostFile {
        let entry = &self.entries[name];
        BoostFile {
            name: name.clone(),
            host: name.host().to_owned(),
            kind: name.kind(),
            code: entry.code.clone(),
            error: entry.compiled.as_ref().and_then(|c| c.error.clone()),
        }
    }
}

/// `create`'s check: `/^[a-z0-9-]+(\.[a-z0-9-]+)*$/`.
fn is_new_boost_host(host: &str) -> bool {
    host.split('.').all(|label| {
        !label.is_empty()
            && label
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}

/// `save`'s check: `/^[a-z0-9.-]+$/i`.
fn is_saved_host(host: &str) -> bool {
    !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
}

/// `TEMPLATES`: a new Boost's text.
fn template(host: &str, kind: BoostKind) -> String {
    match kind {
        BoostKind::Script => format!(
            "// Boost for {host}: runs on every page of this site once it has loaded.\n\
             // It's TypeScript; Flecha strips the types when you save (\u{2318}S), and open {host} tabs reload.\n\
             \n\
             export {{}};\n\
             \n\
             // document.querySelector('header')?.remove();\n"
        ),
        BoostKind::Styles => format!(
            "/* Boost for {host}: these styles apply to every page of this site, live as you save (\u{2318}S). */\n\
             \n\
             /* body {{ font-family: ui-sans-serif, system-ui; }} */\n"
        ),
    }
}
