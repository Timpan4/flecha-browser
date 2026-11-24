//! Site icons, downloaded by a page's renderer and handed to the shell as pixels.
//!
//! The shell asks for an icon URL ([`Engine::download_favicon`]) and gets exactly one
//! [`Favicon`] back for it ([`Engine::next_favicon`]): the bitmap, or `None` when the download
//! failed or CEF dropped the request with its browser. Results are keyed by partition and URL,
//! not by tab, so a result outlives the browser that fetched it. A download sends its
//! partition's cookies, so its image belongs to that partition alone.

use std::cell::Cell;
use std::fmt;
use std::rc::Rc;

use cef::{
    AlphaType, BinaryValue, CefString, ColorType, DownloadImageCallback, Image,
    ImplBinaryValue as _, ImplBrowser as _, ImplBrowserHost as _, ImplDownloadImageCallback,
    ImplImage as _, WrapDownloadImageCallback, rc::Rc as _, wrap_download_image_callback,
};
use flecha_core::TabId;

use crate::callbacks::Stage;
use crate::services::Services;
use crate::{Engine, Partition};

/// The icon size the shell asks for, in DIPs: the shipping shell's `_favicon/?size=32`
/// (chromium/shell/favicons.ts:8). CEF returns versions up to the display's scale factor.
pub const FAVICON_SIZE: u32 = 32;

/// One download's result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Favicon {
    /// The partition whose browser fetched it, with that partition's cookies.
    pub partition: Partition,
    pub url: String,
    pub image: Option<FaviconBitmap>,
}

/// BGRA pixels with straight alpha, the layout GPUI's `RenderImage` draws.
#[derive(Clone, PartialEq, Eq)]
pub struct FaviconBitmap {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl fmt::Debug for FaviconBitmap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FaviconBitmap")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

impl FaviconBitmap {
    /// Pixels from a CEF bitmap of `width` by `height`, if its length matches.
    fn from_bitmap(width: i32, height: i32, bitmap: &BinaryValue) -> Option<Self> {
        let (width, height) = (u32::try_from(width).ok()?, u32::try_from(height).ok()?);
        let len = usize::try_from(width)
            .ok()?
            .checked_mul(usize::try_from(height).ok()?)?
            .checked_mul(4)?;
        if len == 0 || bitmap.size() != len {
            return None;
        }
        let mut bgra = vec![0; len];
        (bitmap.data(Some(&mut bgra), 0) == len).then_some(Self {
            width,
            height,
            bgra,
        })
    }
}

/// Reports one result for its URL: the bitmap when CEF calls back, or a failure when CEF drops
/// the request without calling (its browser closed first).
struct Request {
    services: Rc<Services>,
    partition: Partition,
    url: String,
    reported: Cell<bool>,
}

impl Request {
    fn report(&self, image: Option<FaviconBitmap>) {
        if !self.reported.replace(true) {
            self.services.favicons.borrow_mut().push_back(Favicon {
                partition: self.partition.clone(),
                url: self.url.clone(),
                image,
            });
        }
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        self.report(None);
    }
}

wrap_download_image_callback! {
    struct Download {
        request: Rc<Request>,
        scale: f32,
    }
    impl DownloadImageCallback {
        // CEF calls this on the browser process UI thread (cef_browser_capi.h).
        fn on_download_image_finished(
            &self, _image_url: Option<&CefString>, _status: i32, image: Option<&mut Image>,
        ) {
            self.request.report(image.and_then(|image| bitmap(image, self.scale)));
        }
    }
}

/// The representation closest to `scale` as straight-alpha BGRA.
fn bitmap(image: &Image, scale: f32) -> Option<FaviconBitmap> {
    if image.is_empty() != 0 {
        return None;
    }
    let (mut width, mut height) = (0, 0);
    let pixels = image.as_bitmap(
        scale,
        ColorType::BGRA_8888,
        AlphaType::POSTMULTIPLIED,
        Some(&mut width),
        Some(&mut height),
    )?;
    FaviconBitmap::from_bitmap(width, height, &pixels)
}

impl Engine {
    /// Downloads the icon at `url` through `tab`'s browser, or another live browser in
    /// `partition` when `tab` has none. The request carries that partition's cookies: the
    /// fixture sees them on icon requests, although cef_browser.h:516 says `is_favicon` sends
    /// none. Returns `false` when no browser can fetch it, and [`Self::next_favicon`] then
    /// reports nothing for it.
    pub fn download_favicon(&self, tab: &TabId, partition: &Partition, url: &str) -> bool {
        let own = self
            .browsers
            .get(tab)
            .filter(|session| &session.partition == partition);
        let slot = own
            .into_iter()
            .chain(
                self.browsers
                    .values()
                    .filter(|session| &session.partition == partition),
            )
            .map(|session| &session.slot)
            .find(|slot| !slot.retired.get() && slot.stage.get() == Stage::Live);
        let Some(slot) = slot else {
            return false;
        };
        let Some(host) = slot.browser.borrow().as_ref().and_then(|b| b.host()) else {
            return false;
        };
        let request = Rc::new(Request {
            services: self.popups.services.clone(),
            partition: partition.clone(),
            url: url.to_owned(),
            reported: Cell::new(false),
        });
        let mut callback = Download::new(request, slot.viewport.get().scale());
        host.download_image(
            Some(&CefString::from(url)),
            1,
            FAVICON_SIZE,
            0,
            Some(&mut callback),
        );
        true
    }

    /// The next finished icon download.
    pub fn next_favicon(&mut self) -> Option<Favicon> {
        self.popups.services.favicons.borrow_mut().pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dropped_request_reports_one_failure() {
        let services = Rc::new(Services::new());
        let request = Request {
            services: services.clone(),
            partition: Partition::Shared,
            url: "https://example.test/icon.png".into(),
            reported: Cell::new(false),
        };
        request.report(Some(FaviconBitmap {
            width: 1,
            height: 1,
            bgra: vec![0; 4],
        }));
        drop(request);
        let dropped = Request {
            services: services.clone(),
            partition: Partition::Shared,
            url: "https://example.test/gone.png".into(),
            reported: Cell::new(false),
        };
        drop(dropped);
        let results: Vec<_> = services.favicons.borrow_mut().drain(..).collect();
        assert_eq!(
            results
                .iter()
                .map(|result| (result.url.as_str(), result.image.is_some()))
                .collect::<Vec<_>>(),
            [
                ("https://example.test/icon.png", true),
                ("https://example.test/gone.png", false),
            ]
        );
    }
}
