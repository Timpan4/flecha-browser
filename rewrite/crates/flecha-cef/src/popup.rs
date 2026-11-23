//! Browsers a page opens: `target=_blank` links, `window.open` and scripted popups.
//!
//! CEF creates each one in its opener's request context, with `window.opener`, the
//! referrer and any POST body intact, as Chromium does for the TypeScript shell. The
//! shell then places the new browser as a tab, a Peek, a window or a popup window.
//! Chromium's popup blocker has already refused any popup without a user gesture
//! before `on_before_popup` runs.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cef::{CefString, Client, PopupFeatures, RuntimeStyle, WindowInfo, WindowOpenDisposition};
use flecha_core::TabId;

use crate::Event;
use crate::callbacks::{self, Slot};
use crate::identity::Counter;

/// Where the page asked its new browser to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    ForegroundTab,
    BackgroundTab,
    Window,
    Popup,
}

impl Disposition {
    fn from_cef(disposition: WindowOpenDisposition) -> Option<Self> {
        // Chromium's IsFlechaNewTab (patch 0023) treats a singleton tab as a new tab.
        if disposition == WindowOpenDisposition::NEW_FOREGROUND_TAB
            || disposition == WindowOpenDisposition::SINGLETON_TAB
        {
            Some(Self::ForegroundTab)
        } else if disposition == WindowOpenDisposition::NEW_BACKGROUND_TAB {
            Some(Self::BackgroundTab)
        } else if disposition == WindowOpenDisposition::NEW_WINDOW {
            Some(Self::Window)
        } else if disposition == WindowOpenDisposition::NEW_POPUP {
            Some(Self::Popup)
        } else {
            None
        }
    }
}

/// A browser CEF is creating for a page. The engine owns it from the first event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    pub url: String,
    pub disposition: Disposition,
    /// The popup's requested content size in CSS pixels, from `window.open` features.
    pub size: Option<(i32, i32)>,
}

pub(crate) struct Pending {
    pub child: Rc<Slot>,
    pub opener: Rc<Slot>,
}

/// State every browser's callbacks share with the engine.
pub(crate) struct Popups {
    pub counter: Counter,
    pub mint: fn() -> TabId,
    /// Popups CEF is creating that the engine hasn't registered yet.
    pub pending: RefCell<Vec<Pending>>,
    /// CEF browsers between `on_after_created` and `on_before_close`.
    pub live: Cell<usize>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn before_popup(
    opener: &Rc<Slot>,
    popup_id: i32,
    url: Option<&CefString>,
    disposition: WindowOpenDisposition,
    features: Option<&PopupFeatures>,
    window: Option<&mut WindowInfo>,
    client: Option<&mut Option<Client>>,
) -> bool {
    let (Some(disposition), Some(window), Some(client)) =
        (Disposition::from_cef(disposition), window, client)
    else {
        return false;
    };
    if opener.retired.get() {
        return false;
    }
    let size = features
        .filter(|features| {
            disposition == Disposition::Popup && features.width_set != 0 && features.height_set != 0
        })
        .map(|features| (features.width.max(1), features.height.max(1)));
    let mut viewport = opener.viewport.get();
    if let Some(size) = size {
        viewport.size = size;
    }
    let shared = opener.popups.clone();
    let key = shared.counter.reserve((shared.mint)());
    let child = Rc::new(Slot::new(key, viewport, opener.events.clone(), shared));
    child
        .origin
        .replace(Some((Rc::downgrade(opener), popup_id)));
    // CEF writes the whole struct back, so change only the fields OSR needs.
    window.windowless_rendering_enabled = 1;
    window.runtime_style = RuntimeStyle::ALLOY;
    *client = Some(callbacks::client(child.clone()));
    child.emit(Event::Opened(Opened {
        url: url.map(ToString::to_string).unwrap_or_default(),
        disposition,
        size,
    }));
    opener.opening.borrow_mut().insert(popup_id, child.clone());
    opener.popups.pending.borrow_mut().push(Pending {
        child,
        opener: opener.clone(),
    });
    true
}

/// CEF gave up creating a popup after `before_popup` allowed it.
pub(crate) fn aborted(opener: &Slot, popup_id: i32) {
    let child = opener.opening.borrow_mut().remove(&popup_id);
    if let Some(child) = child {
        child.closed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_dispositions_map_to_the_shipping_shells_placements() {
        let cases = [
            (
                WindowOpenDisposition::NEW_FOREGROUND_TAB,
                Some(Disposition::ForegroundTab),
            ),
            (
                WindowOpenDisposition::SINGLETON_TAB,
                Some(Disposition::ForegroundTab),
            ),
            (
                WindowOpenDisposition::NEW_BACKGROUND_TAB,
                Some(Disposition::BackgroundTab),
            ),
            (WindowOpenDisposition::NEW_WINDOW, Some(Disposition::Window)),
            (WindowOpenDisposition::NEW_POPUP, Some(Disposition::Popup)),
            (WindowOpenDisposition::CURRENT_TAB, None),
            (WindowOpenDisposition::NEW_PICTURE_IN_PICTURE, None),
            (WindowOpenDisposition::NEW_SPLIT_VIEW, None),
        ];
        for (cef, expected) in cases {
            assert_eq!(Disposition::from_cef(cef), expected, "{cef:?}");
        }
    }
}
