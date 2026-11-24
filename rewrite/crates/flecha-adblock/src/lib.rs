//! Flecha's ad and tracker blocker on adblock-rust, independent of CEF and GPUI.
//!
//! - [`AdBlocker`] answers [`AdBlocker::should_block`] for the CEF resource-request hook and
//!   [`AdBlocker::cosmetic_filters`] for element hiding.
//! - [`Updater`] loads and refreshes EasyList and EasyPrivacy on its own thread and installs
//!   each new engine into the `AdBlocker`.
//! - The per-site switch is `Settings::ad_block_allowed` in flecha-core; pass it to
//!   [`AdBlocker::new`] and [`AdBlocker::set_allowed_sites`].

mod blocker;
mod cosmetic;
pub mod engine;
pub mod fetch;
pub mod lists;
mod sites;
mod transport;
mod updater;

pub use blocker::{AdBlocker, Request, RequestType};
pub use cosmetic::{CosmeticFilters, is_injectable_selector, stylesheet};
pub use fetch::{DOWNLOAD_TIMEOUT, Download, DownloadRequest, MAX_REDIRECTS, WireResponse};
pub use lists::{LISTS, MAX_AGE};
pub use sites::{SiteAllowList, site_host};
pub use transport::UreqDownload;
pub use updater::{
    EngineOutcome, UpdateReport, Updater, UpdaterConfig, load_cached_rules, update_once, warm_start,
};
