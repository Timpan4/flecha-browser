//! Sites where blocking is off. The list itself is `Settings::ad_block_allowed` in flecha-core,
//! which owns its persistence and edits (`chromium/shared/adBlockSites.ts`). This is the
//! read-only copy a request check uses.

use std::sync::Arc;

use flecha_core::{blocks_ads_on, is_site_host};

/// A page's hostname as the shield and the allow check see it: the WHATWG URL parser's, as
/// `new URL(url).hostname` in the Chromium shell. It is lowercase ASCII, with IDNA applied, for
/// special schemes. `None` when the URL doesn't parse.
pub fn site_host(page_url: &str) -> Option<String> {
    let url = url::Url::parse(page_url).ok()?;
    Some(url.host_str().unwrap_or_default().to_owned())
}

/// The valid entries of a saved allow list.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SiteAllowList {
    sites: Arc<[String]>,
}

impl SiteAllowList {
    /// Keeps the entries that are request-domain hostnames and returns how many it dropped.
    /// Entries compare byte for byte, so `Example.com` or a Unicode form is dropped, never
    /// folded into something else.
    pub fn new(entries: &[String]) -> (Self, usize) {
        let sites: Arc<[String]> = entries
            .iter()
            .filter(|host| is_site_host(host))
            .cloned()
            .collect();
        let dropped = entries.len() - sites.len();
        (Self { sites }, dropped)
    }

    /// Whether blocking applies on a page with this hostname.
    pub fn blocks_ads_on(&self, host: &str) -> bool {
        blocks_ads_on(Some(&self.sites), host)
    }

    /// Whether blocking applies on this page.
    pub fn blocks_ads_on_page(&self, page_url: &str) -> bool {
        site_host(page_url).is_none_or(|host| self.blocks_ads_on(&host))
    }

    pub fn sites(&self) -> &[String] {
        &self.sites
    }
}
