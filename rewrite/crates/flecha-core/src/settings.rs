use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::shortcuts::Keybindings;

/// Chromium's uncollapsed vertical tab strip bounds, used by sidebarWidth.ts.
pub const SIDEBAR_MIN_WIDTH: u16 = 126;
pub const SIDEBAR_MAX_WIDTH: u16 = 400;
/// Flecha's kFlechaSidebarWidth.
pub const SIDEBAR_DEFAULT_WIDTH: u16 = 260;

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct SidebarWidth(f64);

impl SidebarWidth {
    pub fn clamp(pixels: f64) -> Option<Self> {
        if pixels.is_nan() {
            return None;
        }
        let rounded = pixels
            .clamp(f64::from(SIDEBAR_MIN_WIDTH), f64::from(SIDEBAR_MAX_WIDTH))
            .round();
        Some(Self(rounded))
    }

    pub(crate) fn from_saved(pixels: f64) -> Option<Self> {
        (f64::from(SIDEBAR_MIN_WIDTH)..=f64::from(SIDEBAR_MAX_WIDTH))
            .contains(&pixels)
            .then_some(Self(pixels))
    }

    pub fn pixels(self) -> f64 {
        self.0
    }
}

impl Default for SidebarWidth {
    fn default() -> Self {
        Self(f64::from(SIDEBAR_DEFAULT_WIDTH))
    }
}

/// The presets in Flecha Settings. Existing profiles can also hold custom hours.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(transparent)]
pub struct ArchivePolicy(f64);

impl ArchivePolicy {
    pub const HALF_DAY: Self = Self(12.0);
    pub const DAY: Self = Self(24.0);
    pub const WEEK: Self = Self(7.0 * Self::DAY.0);
    pub const THIRTY_DAYS: Self = Self(30.0 * Self::DAY.0);
    pub const NEVER: Self = Self(0.0);

    pub fn from_hours(hours: f64) -> Option<Self> {
        (hours.is_finite() && hours >= 0.0).then_some(Self(hours))
    }

    pub fn hours(self) -> f64 {
        self.0
    }
}

impl Default for ArchivePolicy {
    fn default() -> Self {
        Self::HALF_DAY
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchEngine {
    #[default]
    Google,
    DuckDuckGo,
    Bing,
    Brave,
    Kagi,
    Ecosia,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentTarget {
    pub label: String,
    pub origin: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_label: Option<bool>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Environment {
    pub id: String,
    pub name: String,
    pub targets: Vec<EnvironmentTarget>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maximized: Option<bool>,
}

/// The MCP bridge for coding agents (`agent` in `chromium/shared/types.d.ts`). The shell kept
/// the bearer token beside `enabled`; the rewrite keeps it in its own file
/// (`flecha_services::agent::AgentToken`), so the saved state carries only the switch.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct AgentPreference {
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionDecision {
    Allow,
    Block,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub theme: Theme,
    pub search_engine: SearchEngine,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_suggest_enabled: Option<bool>,
    pub sidebar_width: SidebarWidth,
    #[serde(rename = "archiveAfterHours")]
    pub archive_policy: ArchivePolicy,
    pub keybindings: Keybindings,
    pub environments: Vec<Environment>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window: Option<WindowBounds>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub peek_pinned_links: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_links_in_flechinha: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sidebar_in_page_fullscreen: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloads_folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ask_where_to_save: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ad_block: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ad_block_allowed: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<AgentPreference>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub onboarded: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub site_permissions: Option<BTreeMap<String, BTreeMap<String, PermissionDecision>>>,
}

/// Hostnames accepted by Chromium's requestDomains rules.
pub fn is_site_host(host: &str) -> bool {
    host.split('.').all(|label| {
        !label.is_empty()
            && label
                .bytes()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == b'-')
    })
}

fn covers_site(site: &str, host: &str) -> bool {
    host == site
        || host
            .strip_suffix(site)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// Whether ads are blocked on `host`, given the sites where blocking is off. An entry covers
/// its subdomains. Hosts compare byte for byte: `host` must already be a URL parser's
/// lowercase ASCII hostname.
pub fn blocks_ads_on(allowed: Option<&[String]>, host: &str) -> bool {
    allowed.is_none_or(|allowed| !allowed.iter().any(|site| covers_site(site, host)))
}

impl Settings {
    pub fn search_suggestions_enabled(&self) -> bool {
        self.search_suggest_enabled.unwrap_or(true)
    }

    /// Whether links from outside Flecha open in a Flechinha (`openLinksInFlechinha !== false`
    /// in `chromium/shell/lib/settingsCore.ts`).
    pub fn opens_links_in_flechinha(&self) -> bool {
        self.open_links_in_flechinha.unwrap_or(true)
    }

    /// Whether the sidebar stays beside a page in fullscreen (`!!sidebarInPageFullscreen`).
    pub fn keeps_sidebar_in_page_fullscreen(&self) -> bool {
        self.sidebar_in_page_fullscreen.unwrap_or(false)
    }

    /// Whether coding agents may use Flecha: off unless turned on in Settings.
    pub fn agents_enabled(&self) -> bool {
        self.agent.is_some_and(|agent| agent.enabled)
    }

    /// The per-site rule. The global ad-block switch is stored separately.
    pub fn blocks_ads_on(&self, host: &str) -> bool {
        blocks_ads_on(self.ad_block_allowed.as_deref(), host)
    }

    pub fn set_ad_blocking_on(&mut self, host: &str, block: bool) -> bool {
        if !is_site_host(host) {
            return false;
        }
        let append = !block && self.blocks_ads_on(host);
        let allowed = self.ad_block_allowed.get_or_insert_with(Vec::new);
        if block {
            allowed.retain(|site| !covers_site(site, host));
        } else if append {
            allowed.push(host.into());
        }
        true
    }
}
