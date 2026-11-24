//! Site icons for tab rows, Favorites, the command bar and the site panel.
//!
//! A page's document declares its icons ([`flecha_cef::Event::FaviconUrls`]). The first one
//! that downloads becomes the tab's `favicon`, which storage keeps, as the shipping shell
//! keeps `TabInfo.favicon` (chromium/shell/ui/app.ts:107-124). After a restart a tab with a
//! saved icon URL gets its image through any live browser in its own partition. Images stay
//! cached only while a tab refers to their URL, so the cache never outgrows the open tabs.
//! Every icon falls back to the site's letter while it loads or when the site has none.
//!
//! A download carries its partition's cookies, so its image can depend on who is signed in.
//! Everything here is keyed by partition as well as URL: no image, failure or pending download
//! crosses from one partition to another. Incognito entries live in memory only and go with the
//! tabs that refer to them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use flecha_cef::{Favicon, FaviconBitmap, Partition};
use flecha_core::{Placement, TabId, WindowId};
use gpui::RenderImage;

use crate::Controller;
use crate::view::site;

/// An icon URL as one partition fetches it.
type Key = (Partition, String);

#[derive(Default)]
pub(crate) struct Favicons {
    images: BTreeMap<Key, Arc<RenderImage>>,
    /// Icons whose download failed. A page that declares one again retries it.
    failed: BTreeSet<Key>,
    /// Icons asked of CEF, each until its one [`Favicon`] result arrives.
    downloading: BTreeSet<Key>,
    /// Icons a page declared that are not settled yet, in the page's order, with the partition
    /// the page browses in.
    declared: BTreeMap<TabId, (Partition, Vec<String>)>,
    /// Whether tabs or browsers may have changed since the last sweep.
    stale: bool,
}

fn key(partition: &Partition, url: &str) -> Key {
    (partition.clone(), url.to_owned())
}

impl Favicons {
    /// The image `partition` downloaded for an icon URL.
    fn image(&self, partition: &Partition, url: &str) -> Option<Arc<RenderImage>> {
        self.images.get(&key(partition, url)).cloned()
    }

    /// What the page's declared icons settle to now: `Some(icon)` when one is cached or every
    /// one failed (`Some(None)`), `None` while the first one still left to try downloads.
    fn settle(&self, partition: &Partition, declared: &[String]) -> Option<Option<String>> {
        for url in declared {
            let key = key(partition, url);
            if self.images.contains_key(&key) {
                return Some(Some(url.clone()));
            }
            if !self.failed.contains(&key) {
                return None;
            }
        }
        Some(None)
    }

    /// The first declared icon that is neither cached nor failed in `partition`.
    fn next_wanted(&self, partition: &Partition, declared: &[String]) -> Option<String> {
        declared
            .iter()
            .find(|url| {
                let key = key(partition, url);
                !self.images.contains_key(&key) && !self.failed.contains(&key)
            })
            .cloned()
    }

    fn store(&mut self, result: Favicon) {
        let key = (result.partition, result.url);
        self.downloading.remove(&key);
        match result.image.and_then(render_image) {
            Some(image) => {
                self.failed.remove(&key);
                self.images.insert(key, Arc::new(image));
            }
            None => {
                self.failed.insert(key);
            }
        }
    }

    /// Forgets icons no tab refers to any more.
    fn retain(&mut self, referenced: &BTreeSet<Key>) {
        self.images.retain(|key, _| referenced.contains(key));
        self.failed.retain(|key| referenced.contains(key));
    }

    pub(crate) fn clear_partition(&mut self, partition: &Partition) {
        self.images.retain(|(owner, _), _| owner != partition);
        self.failed.retain(|(owner, _)| owner != partition);
        self.downloading.retain(|(owner, _)| owner != partition);
        self.declared.retain(|_, (owner, _)| owner != partition);
    }
}

/// GPUI's image from straight-alpha BGRA pixels.
fn render_image(bitmap: FaviconBitmap) -> Option<RenderImage> {
    let buffer = image::RgbaImage::from_raw(bitmap.width, bitmap.height, bitmap.bgra)?;
    Some(RenderImage::new(vec![image::Frame::new(buffer)]))
}

impl Controller {
    /// Favorites are shown in every Space. Their icon must come from the viewing Space.
    pub(crate) fn favicon_partition(&self, window: &WindowId, id: &TabId) -> Option<Partition> {
        if self.model.core.locate(id) == Some(Placement::Favorite) {
            let space = &self.model.core.window(window)?.active_space_id;
            return Some(self.model.window_partition(window, space));
        }
        self.model.partition(id)
    }

    pub(crate) fn tab_favicon(&self, window: &WindowId, id: &TabId) -> Option<Arc<RenderImage>> {
        let tab = self.model.tab(id)?;
        let partition = self.favicon_partition(window, id)?;
        let own = tab
            .favicon
            .as_deref()
            .and_then(|url| self.favicons.image(&partition, url));
        if own.is_none() && self.model.partition(id).as_ref() != Some(&partition) {
            return self.site_favicon(&partition, &tab.url);
        }
        own
    }

    /// The icon of a tab in `partition` on the same site as `url`, for addresses with no tab of
    /// their own (the command bar's history results), as `faviconHosts` does in
    /// ui/app.ts:116-123.
    pub(crate) fn site_favicon(
        &self,
        partition: &Partition,
        url: &str,
    ) -> Option<Arc<RenderImage>> {
        let host = site(url);
        self.model
            .all_tabs()
            .filter(|(_, tab)| site(&tab.url) == host)
            .filter(|(id, _)| self.model.partition(id).as_ref() == Some(partition))
            .find_map(|(_, tab)| self.favicons.image(partition, tab.favicon.as_deref()?))
    }

    /// A page declared its icons. Returns whether the tab changed and storage keeps it.
    pub(crate) fn page_favicons(&mut self, id: &TabId, urls: Vec<String>) -> (bool, bool) {
        let Some(partition) = self.model.partition(id) else {
            return (false, false);
        };
        self.favicons
            .failed
            .retain(|(failed, url)| failed != &partition || !urls.contains(url));
        self.favicons.declared.insert(id.clone(), (partition, urls));
        self.settle_favicon(id)
    }

    /// The tab started loading a new document, which declares its own icons: the last
    /// document's pending ones no longer apply.
    pub(crate) fn page_loading(&mut self, id: &TabId) {
        self.favicons.declared.remove(id);
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
        let Some((partition, declared)) = self.favicons.declared.get(id) else {
            return (false, false);
        };
        let Some(icon) = self.favicons.settle(partition, declared) else {
            if let Some(url) = self.favicons.next_wanted(partition, declared) {
                let partition = partition.clone();
                self.request_favicon(id, &partition, url);
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

    fn request_favicon(&mut self, id: &TabId, partition: &Partition, url: String) {
        let key = (partition.clone(), url);
        if self.favicons.downloading.contains(&key) {
            return;
        }
        if self.engine.download_favicon(id, partition, &key.1) {
            self.favicons.downloading.insert(key);
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
            let (Some(url), Some(partition)) = (&tab.favicon, self.model.partition(id)) else {
                continue;
            };
            let key = (partition, url.clone());
            if !self.favicons.images.contains_key(&key)
                && !self.favicons.failed.contains(&key)
                && !self.favicons.downloading.contains(&key)
            {
                wanted.push((id.clone(), key.clone()));
            }
            referenced.insert(key);
        }
        self.favicons.declared.retain(|id, _| ids.contains(id));
        referenced.extend(
            self.favicons
                .declared
                .values()
                .flat_map(|(partition, urls)| urls.iter().map(|url| key(partition, url))),
        );
        self.favicons.retain(&referenced);
        for (id, (partition, url)) in wanted {
            self.request_favicon(&id, &partition, url);
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
            partition: Partition::Shared,
            url: url.into(),
            image,
        }
    }

    #[test]
    fn closing_the_last_incognito_window_drops_its_favicon_cache_immediately() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let first = controller.model.open_incognito_window();
        let last = controller.model.open_incognito_window();
        let partition = Partition::Incognito(controller.model.incognito_session(&last).unwrap());
        let url = "https://example.test/icon.png";
        let failed = "https://example.test/failed.png";
        let pending = "https://example.test/pending.png";
        let tab = controller
            .model
            .open(&last, "https://example.test/".into(), true)
            .unwrap();
        controller.favicons.store(result(url, Some(bitmap())));
        for (url, image) in [(url, Some(bitmap())), (failed, None)] {
            controller.favicons.store(Favicon {
                partition: partition.clone(),
                url: url.into(),
                image,
            });
        }
        controller
            .favicons
            .downloading
            .insert(key(&partition, pending));
        controller
            .favicons
            .declared
            .insert(tab.clone(), (partition.clone(), vec![pending.into()]));

        controller.close_incognito_window(&first);
        assert!(controller.favicons.image(&partition, url).is_some());
        controller.close_incognito_window(&last);
        assert!(controller.favicons.image(&partition, url).is_none());
        assert!(
            !controller
                .favicons
                .failed
                .contains(&key(&partition, failed))
        );
        assert!(
            !controller
                .favicons
                .downloading
                .contains(&key(&partition, pending))
        );
        assert!(!controller.favicons.declared.contains_key(&tab));
        assert!(controller.favicons.image(&Partition::Shared, url).is_some());
    }

    #[test]
    fn a_favorite_uses_the_viewing_spaces_favicon_partition() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        let favorite = controller
            .model
            .open(&window, "https://example.test/".into(), true)
            .unwrap();
        controller
            .act(
                &window,
                crate::Action::Sidebar(crate::sidebar::SidebarAction::Favorite(favorite.clone())),
            )
            .unwrap();
        let url = "https://example.test/icon.png";
        controller.model.tab_mut(&favorite).unwrap().0.favicon = Some(url.into());
        controller.favicons.store(result(url, Some(bitmap())));
        let shared = controller.tab_favicon(&window, &favorite).unwrap();

        let isolated = crate::model::space_id();
        assert!(controller.model.core.add_space(isolated.clone()));
        controller.model.core.set_isolated(&isolated, true);
        assert!(
            controller
                .model
                .core
                .switch_space(&window, &isolated)
                .result
        );
        let partition = Partition::Isolated(isolated);
        assert_eq!(
            controller.favicon_partition(&window, &favorite),
            Some(partition.clone())
        );
        assert!(controller.tab_favicon(&window, &favorite).is_none());
        controller.favicons.store(Favicon {
            partition: partition.clone(),
            url: url.into(),
            image: Some(bitmap()),
        });
        let isolated = controller.tab_favicon(&window, &favorite).unwrap();
        assert_eq!(
            isolated.id,
            controller.favicons.image(&partition, url).unwrap().id
        );
        assert_ne!(isolated.id, shared.id);
    }

    #[test]
    fn declared_icons_settle_on_the_first_that_downloads() {
        let mut favicons = Favicons::default();
        let shared = Partition::Shared;
        let declared = vec![
            "https://a.test/icon.svg".to_owned(),
            "https://a.test/favicon.ico".to_owned(),
        ];
        assert_eq!(favicons.settle(&shared, &declared), None);
        assert_eq!(
            favicons.next_wanted(&shared, &declared).as_deref(),
            Some("https://a.test/icon.svg")
        );

        favicons.store(result("https://a.test/icon.svg", None));
        assert_eq!(favicons.settle(&shared, &declared), None);
        assert_eq!(
            favicons.next_wanted(&shared, &declared).as_deref(),
            Some("https://a.test/favicon.ico")
        );

        favicons.store(result("https://a.test/favicon.ico", Some(bitmap())));
        assert_eq!(
            favicons.settle(&shared, &declared),
            Some(Some("https://a.test/favicon.ico".into()))
        );
        let image = favicons
            .image(&shared, "https://a.test/favicon.ico")
            .unwrap();
        assert_eq!(image.as_bytes(0), Some(&bitmap().bgra[..]));
    }

    #[test]
    fn a_page_whose_icons_all_fail_keeps_its_letter() {
        let mut favicons = Favicons::default();
        favicons.store(result("https://b.test/favicon.ico", None));
        assert_eq!(
            favicons.settle(&Partition::Shared, &["https://b.test/favicon.ico".into()]),
            Some(None)
        );
        assert_eq!(favicons.settle(&Partition::Shared, &[]), Some(None));
    }

    #[test]
    fn a_malformed_bitmap_is_a_failure() {
        let mut favicons = Favicons::default();
        let mut short = bitmap();
        short.bgra.pop();
        favicons.store(result("https://c.test/favicon.ico", Some(short)));
        assert!(
            favicons
                .image(&Partition::Shared, "https://c.test/favicon.ico")
                .is_none()
        );
        assert!(
            favicons
                .failed
                .contains(&key(&Partition::Shared, "https://c.test/favicon.ico"))
        );
    }

    #[test]
    fn an_icon_one_partition_downloaded_is_not_another_partitions() {
        let mut favicons = Favicons::default();
        let url = "https://d.test/favicon.ico";
        favicons.store(result(url, Some(bitmap())));
        favicons.store(result("https://d.test/broken.ico", None));
        let declared = [url.to_owned()];
        for other in [
            Partition::Incognito(flecha_cef::IncognitoSession::new(1)),
            Partition::Isolated(crate::model::space_id()),
        ] {
            assert!(favicons.image(&other, url).is_none());
            assert_eq!(favicons.settle(&other, &declared), None);
            assert_eq!(
                favicons.next_wanted(&other, &declared).as_deref(),
                Some(url)
            );
            assert_eq!(
                favicons.settle(&other, &["https://d.test/broken.ico".into()]),
                None,
                "a failure is the partition's own too"
            );
        }
    }

    #[test]
    fn only_referenced_icons_stay_cached() {
        let mut favicons = Favicons::default();
        favicons.store(result("https://kept.test/favicon.ico", Some(bitmap())));
        favicons.store(result("https://gone.test/favicon.ico", Some(bitmap())));
        favicons.store(result("https://gone.test/broken.ico", None));
        favicons.retain(&BTreeSet::from([key(
            &Partition::Shared,
            "https://kept.test/favicon.ico",
        )]));
        assert!(
            favicons
                .image(&Partition::Shared, "https://kept.test/favicon.ico")
                .is_some()
        );
        assert!(
            favicons
                .image(&Partition::Shared, "https://gone.test/favicon.ico")
                .is_none()
        );
        assert!(favicons.failed.is_empty());
    }

    #[test]
    fn a_tab_keeps_its_icon_and_mute_until_its_site_changes() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        controller
            .act(
                &window,
                crate::Action::Open("https://www.site.test/".into()),
            )
            .unwrap();
        let tab = controller.model.active(&window).unwrap().clone();
        let partition = controller.model.partition(&tab).unwrap();
        let icon = "https://site.test/favicon.ico".to_owned();

        // No browser can fetch it yet, so the tab keeps its letter.
        assert_eq!(
            controller.page_favicons(&tab, vec![icon.clone()]),
            (false, false)
        );
        assert_eq!(controller.model.tab(&tab).unwrap().favicon, None);
        // A new document drops what the last one declared.
        controller.page_loading(&tab);
        controller.favicons.store(Favicon {
            partition: partition.clone(),
            url: icon.clone(),
            image: Some(bitmap()),
        });
        assert_eq!(controller.settle_favicon(&tab), (false, false));
        assert_eq!(controller.model.tab(&tab).unwrap().favicon, None);

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
            controller.tab_favicon(&window, &tab).is_some(),
            "a referenced icon stays"
        );
        assert!(
            controller
                .site_favicon(&partition, "https://site.test/elsewhere")
                .is_some(),
            "an address borrows the icon of a tab on its site"
        );
        assert!(
            controller
                .site_favicon(
                    &Partition::Incognito(flecha_cef::IncognitoSession::new(1)),
                    "https://site.test/",
                )
                .is_none(),
            "but only in that tab's partition"
        );

        controller.page_site_changed(&tab);
        assert_eq!(controller.model.tab(&tab).unwrap().favicon, None);
        controller.favicons_stale();
        controller.sweep_favicons();
        assert!(
            controller.favicons.image(&partition, &icon).is_none(),
            "an unreferenced icon goes"
        );

        let toggle = || crate::Action::Tab(tab.clone(), crate::tab_menu::TabAction::ToggleMute);
        controller.act(&window, toggle()).unwrap();
        assert_eq!(controller.model.tab(&tab).unwrap().muted, Some(true));
        controller.act(&window, toggle()).unwrap();
        assert_eq!(controller.model.tab(&tab).unwrap().muted, None);
    }
}
