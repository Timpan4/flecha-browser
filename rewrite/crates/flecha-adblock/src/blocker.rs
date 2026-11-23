//! The handle the CEF resource-request hook and the cosmetic injector call.
//!
//! Every call reads one immutable snapshot: the read lock is held only to clone an `Arc`, and
//! the updater holds the write lock only to swap one in. Matching runs outside the lock, so a
//! list update never makes the CEF IO thread wait for parsing or I/O. adblock-rust's `Engine` is
//! `Send + Sync` without its `single-thread` feature, which this crate leaves off.

use std::{
    collections::HashSet,
    sync::{Arc, PoisonError, RwLock},
};

use adblock::request::Request as EngineRequest;

use crate::{
    cosmetic::{CosmeticFilters, is_injectable_selector},
    engine::Filters,
    sites::SiteAllowList,
};

/// A resource type, as adblock-rust and declarativeNetRequest name them. The CEF hook maps
/// `cef_resource_type_t` onto these.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RequestType {
    /// A top-level navigation. Never blocked: build-adblock.mjs drops `$document` blocking
    /// filters ("Blocking whole sites: not ours to do"), and declarativeNetRequest rules without
    /// resource types skip main frames.
    MainFrame,
    SubFrame,
    Stylesheet,
    Script,
    Image,
    Font,
    Object,
    XmlHttpRequest,
    Ping,
    CspReport,
    Media,
    WebSocket,
    Other,
}

impl RequestType {
    fn as_adblock(self) -> &'static str {
        match self {
            Self::MainFrame => "main_frame",
            Self::SubFrame => "sub_frame",
            Self::Stylesheet => "stylesheet",
            Self::Script => "script",
            Self::Image => "image",
            Self::Font => "font",
            Self::Object => "object",
            Self::XmlHttpRequest => "xmlhttprequest",
            Self::Ping => "ping",
            Self::CspReport => "csp_report",
            Self::Media => "media",
            Self::WebSocket => "websocket",
            Self::Other => "other",
        }
    }
}

/// One request, as the hook sees it.
#[derive(Clone, Copy, Debug)]
pub struct Request<'a> {
    pub url: &'a str,
    /// The document that made the request (the frame's URL). Third-party and `$domain=` options
    /// match against it.
    pub source_url: &'a str,
    /// The tab's top-level page. The per-site switch matches against it, as the Chromium shell's
    /// main-frame `allowAllRequests` rule covers every frame inside an allowed page.
    pub top_level_url: &'a str,
    pub request_type: RequestType,
}

#[derive(Clone)]
struct Snapshot {
    engine: Option<Arc<Filters>>,
    enabled: bool,
    allowed: SiteAllowList,
}

/// Cheap to clone; every clone sees the same engine and settings.
#[derive(Clone)]
pub struct AdBlocker {
    current: Arc<RwLock<Arc<Snapshot>>>,
}

impl AdBlocker {
    /// No engine yet: nothing is blocked until the updater installs one. `enabled` is
    /// `Settings::ad_block != Some(false)` and `allowed` is `Settings::ad_block_allowed`.
    /// Returns how many allowed entries were dropped as invalid.
    pub fn new(enabled: bool, allowed: &[String]) -> (Self, usize) {
        let (allowed, dropped) = SiteAllowList::new(allowed);
        let snapshot = Snapshot {
            engine: None,
            enabled,
            allowed,
        };
        let blocker = Self {
            current: Arc::new(RwLock::new(Arc::new(snapshot))),
        };
        (blocker, dropped)
    }

    fn snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    fn update(&self, change: impl FnOnce(&mut Snapshot)) {
        let mut current = self.current.write().unwrap_or_else(PoisonError::into_inner);
        let mut next = Snapshot::clone(&current);
        change(&mut next);
        let previous = std::mem::replace(&mut *current, Arc::new(next));
        drop(current);
        // Freeing a replaced engine happens after the lock is released, so readers never wait
        // on it.
        drop(previous);
    }

    /// Settings' "Block ads and trackers".
    pub fn set_enabled(&self, enabled: bool) {
        self.update(|snapshot| snapshot.enabled = enabled);
    }

    /// The saved allow list changed. Returns how many entries were dropped as invalid.
    pub fn set_allowed_sites(&self, allowed: &[String]) -> usize {
        let (allowed, dropped) = SiteAllowList::new(allowed);
        self.update(|snapshot| snapshot.allowed = allowed);
        dropped
    }

    pub(crate) fn install(&self, filters: Filters) {
        let filters = Some(Arc::new(filters));
        self.update(|snapshot| snapshot.engine = filters);
    }

    pub fn has_engine(&self) -> bool {
        self.snapshot().engine.is_some()
    }

    /// Whether to cancel the request. Cheap enough for the CEF IO thread: no I/O, no lock held
    /// while matching.
    pub fn should_block(&self, request: &Request<'_>) -> bool {
        let snapshot = self.snapshot();
        let Some(filters) = snapshot.engine.as_deref() else {
            return false;
        };
        if !snapshot.enabled
            || request.request_type == RequestType::MainFrame
            || !snapshot.allowed.blocks_ads_on_page(request.top_level_url)
        {
            return false;
        }
        // Pages and frames the lists exempt whole, as the shell's `allowAllRequests` rules for
        // main and sub frames did: the frame itself and everything inside it.
        let frame_exempt =
            request.request_type == RequestType::SubFrame && filters.exempts_page(request.url);
        if frame_exempt
            || filters.exempts_page(request.top_level_url)
            || (request.source_url != request.top_level_url
                && filters.exempts_page(request.source_url))
        {
            return false;
        }
        let engine = filters.engine();
        let Ok(parsed) = EngineRequest::new(
            request.url,
            request.source_url,
            request.request_type.as_adblock(),
            "get",
        ) else {
            return false;
        };
        engine.check_network_request(&parsed).should_block()
    }

    /// The element hiding for a page. Selectors that could break a stylesheet are left out.
    /// Scriptlets and procedural filters are not returned: no scriptlet resources are loaded,
    /// and the Chromium shell applied neither.
    pub fn cosmetic_filters(&self, page_url: &str) -> CosmeticFilters {
        let snapshot = self.snapshot();
        let Some(filters) = snapshot.engine.as_deref() else {
            return CosmeticFilters::none();
        };
        if !snapshot.enabled || !snapshot.allowed.blocks_ads_on_page(page_url) {
            return CosmeticFilters::none();
        }
        let resources = filters.engine().url_cosmetic_resources(page_url);
        let mut hide_selectors: Vec<String> = resources
            .hide_selectors
            .into_iter()
            .filter(|selector| is_injectable_selector(selector))
            .collect();
        hide_selectors.sort();
        CosmeticFilters {
            hide_selectors,
            exceptions: resources.exceptions,
            generic_hide: resources.generichide,
        }
    }

    /// Generic class and id selectors that match what the page has, given the exceptions from
    /// [`Self::cosmetic_filters`]. Call it only when that result left `generic_hide` unset: the
    /// engine can't tell from classes and ids alone that a list exempts the page.
    pub fn hidden_class_id_selectors(
        &self,
        page_url: &str,
        classes: &[String],
        ids: &[String],
        exceptions: &HashSet<String>,
    ) -> Vec<String> {
        let snapshot = self.snapshot();
        let Some(filters) = snapshot.engine.as_deref() else {
            return Vec::new();
        };
        if !snapshot.enabled || !snapshot.allowed.blocks_ads_on_page(page_url) {
            return Vec::new();
        }
        let mut selectors: Vec<String> = filters
            .engine()
            .hidden_class_id_selectors(classes, ids, exceptions)
            .into_iter()
            .filter(|selector| is_injectable_selector(selector))
            .collect();
        selectors.sort();
        selectors
    }
}

impl std::fmt::Debug for AdBlocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let snapshot = self.snapshot();
        f.debug_struct("AdBlocker")
            .field("has_engine", &snapshot.engine.is_some())
            .field("enabled", &snapshot.enabled)
            .field("allowed", &snapshot.allowed)
            .finish()
    }
}
