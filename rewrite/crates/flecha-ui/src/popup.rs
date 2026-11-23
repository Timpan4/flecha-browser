//! Placing the browsers pages open, and taking them down when pages close them.

use anyhow::{Context as _, Result};
use flecha_cef::{Disposition, Opened, Owner};
use flecha_core::{Placement, TabId, WindowId, suggestions::pinnable_address};

use crate::Controller;
use crate::model::{Model, now};

/// Where a browser a page opened goes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Place {
    Tab {
        window: WindowId,
        after: Option<TabId>,
        activate: bool,
    },
    Peek(WindowId),
    Window,
    Popup(WindowId),
}

/// The shipping shell's placement. A tab's new tabs follow it in Today
/// (`background.ts` `onCreated`), and a pinned tab's or favorite's foreground links
/// open in Peek (patch 0026). A Peek's new tabs open in its window as foreground tabs
/// unless they ask for the background (patch 0026). A Flechinha's or popup's new tabs
/// open in the main window (patch 0023). Scripted popups stay popups everywhere.
pub(crate) fn place(
    model: &Model,
    owner: &Owner,
    opener: &TabId,
    disposition: Disposition,
) -> Place {
    let (Owner::Tab(window)
    | Owner::Peek(window)
    | Owner::Flechinha(window)
    | Owner::Popup(window)) = owner;
    if disposition == Disposition::Popup {
        return Place::Popup(window.clone());
    }
    let foreground = disposition != Disposition::BackgroundTab;
    match owner {
        Owner::Tab(_) => {
            if disposition == Disposition::Window {
                return Place::Window;
            }
            let pinned = matches!(
                model.core.locate(opener),
                Some(Placement::Pinned { .. } | Placement::Favorite)
            );
            if foreground && pinned && model.core.settings.peek_pinned_links != Some(false) {
                return Place::Peek(window.clone());
            }
            Place::Tab {
                window: window.clone(),
                after: Some(opener.clone()),
                activate: foreground,
            }
        }
        Owner::Peek(_) => Place::Tab {
            window: window.clone(),
            after: None,
            activate: foreground,
        },
        Owner::Flechinha(_) | Owner::Popup(_) => {
            if disposition == Disposition::Window {
                return Place::Window;
            }
            Place::Tab {
                window: model.core.focused_window().id.clone(),
                after: None,
                activate: foreground,
            }
        }
    }
}

impl Controller {
    /// CEF is creating `child` for a page. It already shares its opener's cookie jar.
    pub(crate) fn opened(&mut self, child: &TabId, opened: Opened) -> Result<()> {
        let source = self
            .engine
            .opener(child)
            .and_then(|opener| Some((self.engine.owner(&opener)?.clone(), opener)));
        let Some((owner, opener)) = source.filter(|_| !self.quitting) else {
            self.engine.close(child);
            return Ok(());
        };
        // The shell loads this address itself when it reopens the page in another
        // cookie jar or restores it, so it takes only what the address bar would.
        // CEF still takes the page wherever it asked.
        let url = pinnable_address(&opened.url).unwrap_or_else(|| "about:blank".to_owned());
        let (owner, durable) = match place(&self.model, &owner, &opener, opened.disposition) {
            Place::Tab {
                window,
                after,
                activate,
            } => {
                self.model
                    .open_page(child.clone(), &window, url, after.as_ref(), activate)?;
                (Owner::Tab(window), true)
            }
            Place::Peek(window) => {
                if let Some(old) =
                    self.model
                        .peek_page(window.clone(), child.clone(), url, Some(&opener))?
                {
                    self.engine.close(&old);
                }
                (Owner::Peek(window), false)
            }
            Place::Window => {
                let window = self.model.open_window_page(child.clone(), url)?;
                self.front = Some(window.clone());
                (Owner::Tab(window), true)
            }
            Place::Popup(window) => {
                let partition = self
                    .engine
                    .partition(child)
                    .context("popup lacks a request context")?
                    .clone();
                let popup = self.model.open_popup(
                    child.clone(),
                    url,
                    (&window, &opener),
                    partition,
                    opened.size,
                )?;
                self.front = Some(popup.clone());
                (Owner::Popup(popup), false)
            }
        };
        self.engine.transfer(child, child.clone(), owner);
        // A tab in a Space with another cookie jar reopens there at the address it
        // was going to, as `checkOpenedTab` does.
        self.reconcile_partition(child);
        self.update_visibility();
        if durable { self.save() } else { Ok(()) }
    }

    /// The page closed its own browser, or CEF abandoned creating it.
    pub(crate) fn page_closed(&mut self, id: &TabId) -> Result<()> {
        let popup = self
            .model
            .popups
            .iter()
            .find(|(_, popup)| &popup.page.id == id)
            .map(|(window, _)| window.clone());
        if let Some(window) = popup {
            self.close_popup(&window);
            return Ok(());
        }
        self.model.peeks.retain(|_, page| &page.id != id);
        self.model.flechinhas.retain(|_, page| &page.id != id);
        if self.model.core.tab(id).is_none() {
            return Ok(());
        }
        for removed in self.model.core.close_tab(id, now()).removed {
            self.engine.close(&removed);
        }
        self.update_visibility();
        self.save()
    }

    /// Focus goes back to the window that opened the popup, as when Chromium closes one.
    pub(crate) fn close_popup(&mut self, window: &WindowId) {
        if let Some(popup) = self.model.popups.remove(window) {
            self.engine.close(&popup.page.id);
            if self.model.core.window(&popup.opener).is_some()
                || self.model.flechinhas.contains_key(&popup.opener)
                || self.model.popups.contains_key(&popup.opener)
            {
                self.front = Some(popup.opener);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{tab_id, window_id};
    use flecha_cef::Partition;

    fn model() -> (Model, WindowId, TabId) {
        let mut model = Model::new(None);
        let window = model.core.focused_window().id.clone();
        let tab = model
            .open(&window, "https://example.test/opener".into(), true)
            .unwrap();
        (model, window, tab)
    }

    #[test]
    fn a_tabs_links_open_after_it_and_its_popups_stay_popups() {
        let (model, window, opener) = model();
        let owner = Owner::Tab(window.clone());
        for (disposition, activate) in [
            (Disposition::ForegroundTab, true),
            (Disposition::BackgroundTab, false),
        ] {
            assert_eq!(
                place(&model, &owner, &opener, disposition),
                Place::Tab {
                    window: window.clone(),
                    after: Some(opener.clone()),
                    activate
                }
            );
        }
        assert_eq!(
            place(&model, &owner, &opener, Disposition::Popup),
            Place::Popup(window.clone())
        );
        assert_eq!(
            place(&model, &owner, &opener, Disposition::Window),
            Place::Window
        );
    }

    #[test]
    fn pinned_links_peek_unless_the_setting_is_off() {
        let (mut model, window, _) = model();
        let favorite = tab_id();
        assert!(model.core.create_tab(
            favorite.clone(),
            flecha_core::Tab::new("https://example.test/favorite", now()),
            Placement::Favorite,
            None,
        ));
        let owner = Owner::Tab(window.clone());
        assert_eq!(
            place(&model, &owner, &favorite, Disposition::ForegroundTab),
            Place::Peek(window.clone())
        );
        assert!(matches!(
            place(&model, &owner, &favorite, Disposition::BackgroundTab),
            Place::Tab {
                activate: false,
                ..
            }
        ));
        model.core.settings.peek_pinned_links = Some(false);
        assert!(matches!(
            place(&model, &owner, &favorite, Disposition::ForegroundTab),
            Place::Tab { activate: true, .. }
        ));
    }

    #[test]
    fn popups_from_transient_pages_keep_their_space() {
        let (mut model, window, _) = model();
        let space = model.core.focused_window().active_space_id.clone();
        let peek = model
            .peek(window.clone(), "https://example.test/peek".into(), None)
            .unwrap();
        assert!(peek.is_none());
        let peek = model.peeks[&window].id.clone();
        let tab = model
            .open(&window, "https://example.test/tab".into(), true)
            .unwrap();
        let flechinha = model.detach(&window, &tab).unwrap();
        let first = model
            .open_popup(
                tab_id(),
                "https://example.test/first".into(),
                (&flechinha.window, &flechinha.tab),
                Partition::Shared,
                None,
            )
            .unwrap();
        let first_page = model.popups[&first].page.id.clone();
        let openers = [
            (window.clone(), peek),
            (flechinha.window, flechinha.tab),
            (first, first_page),
        ];
        for (opener_window, opener) in openers {
            let popup = model
                .open_popup(
                    tab_id(),
                    "https://example.test/popup".into(),
                    (&opener_window, &opener),
                    Partition::Shared,
                    Some((480, 360)),
                )
                .unwrap();
            assert_eq!(model.popups[&popup].page.space, space);
            assert_eq!(model.popups[&popup].opener, opener_window);
        }
    }

    #[test]
    fn transient_pages_send_new_tabs_to_a_main_window() {
        let (model, window, opener) = model();
        let peek = Owner::Peek(window.clone());
        assert_eq!(
            place(&model, &peek, &opener, Disposition::Window),
            Place::Tab {
                window: window.clone(),
                after: None,
                activate: true
            }
        );
        for owner in [Owner::Flechinha(window_id()), Owner::Popup(window_id())] {
            assert_eq!(
                place(&model, &owner, &opener, Disposition::BackgroundTab),
                Place::Tab {
                    window: window.clone(),
                    after: None,
                    activate: false
                }
            );
        }
    }
}
