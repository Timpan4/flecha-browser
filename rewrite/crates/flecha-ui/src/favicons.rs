//! Site icons for tab rows, Favorites, the command bar and the site panel.
//!
//! A page's document declares its icons ([`flecha_cef::Event::FaviconUrls`]). The first one
//! that downloads becomes the tab's `favicon`, which storage keeps, as the shipping shell
//! keeps `TabInfo.favicon` (chromium/shell/ui/app.ts:107-124). After a restart a tab with a
//! saved icon URL gets its image through any live browser in its own partition. Images stay
//! cached only while a tab refers to their URL, so the cache never outgrows the open tabs.
//! Every icon falls back to the site's letter while it loads or when the site has none.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use flecha_cef::{Favicon, FaviconBitmap};
use flecha_core::{Tab, TabId};
use gpui::RenderImage;

use crate::Controller;
use crate::view::site;

#[derive(Default)]
pub(crate) struct Favicons {
    images: HashMap<String, Arc<RenderImage>>,
    /// URLs whose download failed. A page that declares one again retries it.
    failed: BTreeSet<String>,
    /// URLs asked of CEF, each until its one [`Favicon`] result arrives.
    downloading: BTreeSet<String>,
    /// Icons a page declared that are not settled yet, in the page's order.
    declared: BTreeMap<TabId, Vec<String>>,
    /// Whether tabs or browsers may have changed since the last sweep.
    stale: bool,
}

impl Favicons {
    /// The cached image for an icon URL.
    pub(crate) fn image(&self, url: Option<&str>) -> Option<Arc<RenderImage>> {
        self.images.get(url?).cloned()
    }

    /// The tab's own icon.
    pub(crate) fn for_tab(&self, tab: &Tab) -> Option<Arc<RenderImage>> {
        self.image(tab.favicon.as_deref())
    }

    /// The icon of a regular tab on the same site as `url`, for addresses with no tab of their
    /// own (the command bar's history results), as `faviconHosts` does in ui/app.ts:116-123.
    pub(crate) fn for_site<'a>(
        &self,
        tabs: impl IntoIterator<Item = &'a Tab>,
        url: &str,
    ) -> Option<Arc<RenderImage>> {
        let host = site(url);
        tabs.into_iter()
            .filter(|tab| site(&tab.url) == host)
            .find_map(|tab| self.for_tab(tab))
    }

    /// What the page's declared icons settle to now: `Some(icon)` when one is cached or every
    /// one failed (`Some(None)`), `None` while the first one still left to try downloads.
    fn settle(&self, declared: &[String]) -> Option<Option<String>> {
        for url in declared {
            if self.images.contains_key(url) {
                return Some(Some(url.clone()));
            }
            if !self.failed.contains(url) {
                return None;
            }
        }
        Some(None)
    }

    /// The first declared icon that is neither cached nor failed.
    fn next_wanted(&self, declared: &[String]) -> Option<String> {
        declared
            .iter()
            .find(|url| !self.images.contains_key(*url) && !self.failed.contains(*url))
            .cloned()
    }

    fn store(&mut self, result: Favicon) {
        self.downloading.remove(&result.url);
        match result.image.and_then(render_image) {
            Some(image) => {
                self.failed.remove(&result.url);
                self.images.insert(result.url, Arc::new(image));
            }
            None => {
                self.failed.insert(result.url);
            }
        }
    }

    /// Forgets icons no tab refers to any more.
    fn retain(&mut self, referenced: &BTreeSet<String>) {
        self.images.retain(|url, _| referenced.contains(url));
        self.failed.retain(|url| referenced.contains(url));
    }
}

/// GPUI's image from straight-alpha BGRA pixels.
fn render_image(bitmap: FaviconBitmap) -> Option<RenderImage> {
    let buffer = image::RgbaImage::from_raw(bitmap.width, bitmap.height, bitmap.bgra)?;
    Some(RenderImage::new(vec![image::Frame::new(buffer)]))
}

impl Controller {
    /// A page declared its icons. Returns whether the tab changed and storage keeps it.
    pub(crate) fn page_favicons(&mut self, id: &TabId, urls: Vec<String>) -> (bool, bool) {
        self.favicons.failed.retain(|url| !urls.contains(url));
        self.favicons.declared.insert(id.clone(), urls);
        self.settle_favicon(id)
    }

    /// The tab's page moved to another site: its icon belongs to the old one.
    pub(crate) fn page_site_changed(&mut self, id: &TabId) {
        self.favicons.declared.remove(id);
        if let Some((tab, _)) = self.model.tab_mut(id) {
            tab.favicon = None;
        }
    }

    /// Applies finished downloads. Returns whether any tab changed, and whether storage keeps
    /// one that did.
    pub(crate) fn drain_favicons(&mut self) -> (bool, bool) {
        let (mut changed, mut save) = (false, false);
        while let Some(result) = self.engine.next_favicon() {
            self.favicons.store(result);
            changed = true;
        }
        if changed {
            let pending: Vec<TabId> = self.favicons.declared.keys().cloned().collect();
            for id in pending {
                let (tab_changed, durable) = self.settle_favicon(&id);
                save |= tab_changed && durable;
            }
        }
        (changed, save)
    }

    /// Sets the tab's icon once its declared icons settle, and asks for the next one to try
    /// otherwise.
    fn settle_favicon(&mut self, id: &TabId) -> (bool, bool) {
        let Some(declared) = self.favicons.declared.get(id) else {
            return (false, false);
        };
        let Some(icon) = self.favicons.settle(declared) else {
            if let Some(url) = self.favicons.next_wanted(declared) {
                self.request_favicon(id, url);
            }
            return (false, false);
        };
        self.favicons.declared.remove(id);
        match self.model.tab_mut(id) {
            Some((tab, durable)) if tab.favicon != icon => {
                tab.favicon = icon;
                (true, durable)
            }
            _ => (false, false),
        }
    }

    fn request_favicon(&mut self, id: &TabId, url: String) {
        if self.favicons.downloading.contains(&url) {
            return;
        }
        let Some(partition) = self.model.partition(id) else {
            return;
        };
        if self.engine.download_favicon(id, &partition, &url) {
            self.favicons.downloading.insert(url);
        }
    }

    /// Marks the tabs as possibly changed, for the next [`Self::sweep_favicons`].
    pub(crate) fn favicons_stale(&mut self) {
        self.favicons.stale = true;
    }

    /// Fetches saved icons that have no image yet, such as a pinned tab's after a restart, and
    /// forgets images no tab refers to. Does nothing unless tabs or browsers changed since the
    /// last sweep.
    pub(crate) fn sweep_favicons(&mut self) {
        if !std::mem::take(&mut self.favicons.stale) {
            return;
        }
        let mut ids = BTreeSet::new();
        let mut referenced = BTreeSet::new();
        let mut wanted = Vec::new();
        for (id, tab) in self.model.all_tabs() {
            ids.insert(id.clone());
            let Some(url) = &tab.favicon else {
                continue;
            };
            referenced.insert(url.clone());
            if !self.favicons.images.contains_key(url)
                && !self.favicons.failed.contains(url)
                && !self.favicons.downloading.contains(url)
            {
                wanted.push((id.clone(), url.clone()));
            }
        }
        self.favicons.declared.retain(|id, _| ids.contains(id));
        referenced.extend(self.favicons.declared.values().flatten().cloned());
        self.favicons.retain(&referenced);
        for (id, url) in wanted {
            self.request_favicon(&id, url);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bitmap() -> FaviconBitmap {
        FaviconBitmap {
            width: 2,
            height: 1,
            bgra: vec![0, 0, 255, 255, 255, 0, 0, 128],
        }
    }

    fn result(url: &str, image: Option<FaviconBitmap>) -> Favicon {
        Favicon {
            url: url.into(),
            image,
        }
    }

    #[test]
    fn declared_icons_settle_on_the_first_that_downloads() {
        let mut favicons = Favicons::default();
        let declared = vec![
            "https://a.test/icon.svg".to_owned(),
            "https://a.test/favicon.ico".to_owned(),
        ];
        assert_eq!(favicons.settle(&declared), None);
        assert_eq!(
            favicons.next_wanted(&declared).as_deref(),
            Some("https://a.test/icon.svg")
        );

        favicons.store(result("https://a.test/icon.svg", None));
        assert_eq!(favicons.settle(&declared), None);
        assert_eq!(
            favicons.next_wanted(&declared).as_deref(),
            Some("https://a.test/favicon.ico")
        );

        favicons.store(result("https://a.test/favicon.ico", Some(bitmap())));
        assert_eq!(
            favicons.settle(&declared),
            Some(Some("https://a.test/favicon.ico".into()))
        );
        let image = favicons.image(Some("https://a.test/favicon.ico")).unwrap();
        assert_eq!(image.as_bytes(0), Some(&bitmap().bgra[..]));
    }

    #[test]
    fn a_page_whose_icons_all_fail_keeps_its_letter() {
        let mut favicons = Favicons::default();
        favicons.store(result("https://b.test/favicon.ico", None));
        assert_eq!(
            favicons.settle(&["https://b.test/favicon.ico".into()]),
            Some(None)
        );
        assert_eq!(favicons.settle(&[]), Some(None));
    }

    #[test]
    fn a_malformed_bitmap_is_a_failure() {
        let mut favicons = Favicons::default();
        let mut short = bitmap();
        short.bgra.pop();
        favicons.store(result("https://c.test/favicon.ico", Some(short)));
        assert!(favicons.image(Some("https://c.test/favicon.ico")).is_none());
        assert!(favicons.failed.contains("https://c.test/favicon.ico"));
    }

    #[test]
    fn only_referenced_icons_stay_cached() {
        let mut favicons = Favicons::default();
        favicons.store(result("https://kept.test/favicon.ico", Some(bitmap())));
        favicons.store(result("https://gone.test/favicon.ico", Some(bitmap())));
        favicons.store(result("https://gone.test/broken.ico", None));
        favicons.retain(&BTreeSet::from(
            ["https://kept.test/favicon.ico".to_owned()],
        ));
        assert!(
            favicons
                .image(Some("https://kept.test/favicon.ico"))
                .is_some()
        );
        assert!(
            favicons
                .image(Some("https://gone.test/favicon.ico"))
                .is_none()
        );
        assert!(favicons.failed.is_empty());
    }

    #[test]
    fn an_address_borrows_the_icon_of_a_tab_on_its_site() {
        let mut favicons = Favicons::default();
        favicons.store(result("https://cdn.test/site.png", Some(bitmap())));
        let mut tab = Tab::new("https://www.site.test/page", 0);
        tab.favicon = Some("https://cdn.test/site.png".into());
        let other = Tab::new("https://other.test/", 0);
        assert!(
            favicons
                .for_site([&other, &tab], "https://site.test/elsewhere")
                .is_some()
        );
        assert!(favicons.for_site([&other], "https://site.test/").is_none());
    }

    #[test]
    fn a_tab_keeps_its_icon_and_mute_until_its_site_changes() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        controller
            .act(&window, crate::Action::Open("https://site.test/".into()))
            .unwrap();
        let tab = controller.model.active(&window).unwrap().clone();
        let icon = "https://site.test/favicon.ico".to_owned();

        // No browser can fetch it yet, so the tab keeps its letter.
        assert_eq!(
            controller.page_favicons(&tab, vec![icon.clone()]),
            (false, false)
        );
        assert_eq!(controller.model.tab(&tab).unwrap().favicon, None);

        controller.favicons.store(result(&icon, Some(bitmap())));
        assert_eq!(
            controller.page_favicons(&tab, vec![icon.clone()]),
            (true, true)
        );
        assert_eq!(
            controller.model.tab(&tab).unwrap().favicon.as_ref(),
            Some(&icon)
        );
        controller.favicons_stale();
        controller.sweep_favicons();
        assert!(
            controller.favicons.image(Some(&icon)).is_some(),
            "a referenced icon stays"
        );

        controller.page_site_changed(&tab);
        assert_eq!(controller.model.tab(&tab).unwrap().favicon, None);
        controller.favicons_stale();
        controller.sweep_favicons();
        assert!(
            controller.favicons.image(Some(&icon)).is_none(),
            "an unreferenced icon goes"
        );

        let toggle = || crate::Action::ToggleMute(tab.clone());
        controller.act(&window, toggle()).unwrap();
        assert_eq!(controller.model.tab(&tab).unwrap().muted, Some(true));
        controller.act(&window, toggle()).unwrap();
        assert_eq!(controller.model.tab(&tab).unwrap().muted, None);
    }
}
