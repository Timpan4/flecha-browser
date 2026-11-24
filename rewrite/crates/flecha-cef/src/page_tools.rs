//! What the shell does to a page on the user's word: copy a picture of it or of one of its
//! images, clear its site's data, and make a responsive mirror behave like a phone. Each goes
//! through the page's own browser, and its outcome comes back as a [`PageToolEvent`].

use std::rc::{Rc, Weak};

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use cef::{
    BinaryValue, CefString, DownloadImageCallback, Image, ImplBinaryValue as _, ImplBrowser as _,
    ImplBrowserHost as _, ImplDownloadImageCallback, ImplFrame as _, ImplImage as _,
    WrapDownloadImageCallback, rc::Rc as _, wrap_download_image_callback,
};
use flecha_core::TabId;

use crate::callbacks::Slot;
use crate::devtools::{Internal, send};
use crate::services;

/// The width a responsive mirror lays out at, in CSS pixels: the shipping phone viewport
/// (chromium/shell/lib/responsiveViewport.ts).
pub const PHONE_WIDTH: u32 = 393;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Copied {
    /// The visible page (the site panel's Screenshot).
    Screenshot,
    /// An image in the page (the page menu's Copy image).
    Image,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageToolEvent {
    /// A PNG for the clipboard, or `None` when the page could not make one.
    Copied { what: Copied, png: Option<Vec<u8>> },
    /// The site's data is gone, and its page loads again, or the clear failed.
    SiteDataCleared { success: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageToolMessage {
    pub tab: TabId,
    pub event: PageToolEvent,
}

/// `Page.captureScreenshot`'s reply: its `data` field, a base64 PNG.
fn screenshot_png(reply: &[u8]) -> Option<Vec<u8>> {
    let reply: serde_json::Value = serde_json::from_slice(reply).ok()?;
    let data = reply.get("data")?.as_str()?;
    base64::engine::general_purpose::STANDARD.decode(data).ok()
}

/// A reply to one of the calls this module makes.
pub(crate) fn reply(
    slot: &Rc<Slot>,
    browser: Option<&mut cef::Browser>,
    call: Tool,
    reply: Option<&[u8]>,
) {
    let event = match call {
        Tool::Screenshot => PageToolEvent::Copied {
            what: Copied::Screenshot,
            png: reply.and_then(screenshot_png),
        },
        Tool::ClearSiteData { origin } => {
            // The page's own scripts and requests still hold what they read; it loads again,
            // unless it has gone to another site since.
            if reply.is_some()
                && let Some(browser) = browser.filter(|_| !slot.retired.get())
                && browser.main_frame().is_some_and(|frame| {
                    on_origin(&CefString::from(&frame.url()).to_string(), &origin)
                })
            {
                browser.reload();
            }
            PageToolEvent::SiteDataCleared {
                success: reply.is_some(),
            }
        }
    };
    services::of(slot)
        .page_tool_events
        .push(Rc::downgrade(slot), event);
}

/// Whether the page at `url` is on `origin`, as [`flecha_permissions::Origin`] serializes it.
fn on_origin(url: &str, origin: &str) -> bool {
    flecha_permissions::Origin::parse(url).is_ok_and(|page| page.as_str() == origin)
}

/// The calls whose replies [`reply`] handles.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Tool {
    Screenshot,
    /// Clears the data of `origin`, the page's origin when it was asked.
    ClearSiteData {
        origin: String,
    },
}

wrap_download_image_callback! {
    struct CopyImage { slot: Weak<Slot> }
    impl DownloadImageCallback {
        fn on_download_image_finished(
            &self, _image_url: Option<&CefString>, _status: i32, image: Option<&mut Image>,
        ) {
            let Some(slot) = self.slot.upgrade() else { return };
            let png = image.and_then(|image| png(image));
            services::of(&slot).page_tool_events.push(
                self.slot.clone(),
                PageToolEvent::Copied { what: Copied::Image, png },
            );
        }
    }
}

fn png(image: &Image) -> Option<Vec<u8>> {
    if image.is_empty() != 0 {
        return None;
    }
    let (mut width, mut height) = (0, 0);
    let bytes: BinaryValue = image.as_png(1.0, 1, Some(&mut width), Some(&mut height))?;
    let mut png = vec![0; bytes.size()];
    (bytes.data(Some(&mut png), 0) == png.len() && !png.is_empty()).then_some(png)
}

/// Electron's `PHONE_USER_AGENT`, as the shipping responsive view sends it
/// (chromium/shell/features/responsive.ts), with this build's Chromium version.
fn phone_user_agent() -> String {
    format!(
        "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) \
         Chrome/{} Mobile Safari/537.36",
        crate::chromium_version()
    )
}

impl crate::Engine {
    fn tool_slot(&self, tab: &TabId) -> Result<(&Rc<Slot>, cef::BrowserHost)> {
        let session = self.browsers.get(tab).context("The page has closed")?;
        let host = self.host(tab).context("The page has closed")?;
        Ok((&session.slot, host))
    }

    /// Copies the visible page as a PNG. The picture arrives as a [`PageToolEvent::Copied`].
    pub fn capture_screenshot(&self, tab: &TabId) -> Result<()> {
        let (slot, host) = self.tool_slot(tab)?;
        send(
            slot,
            &host,
            "Page.captureScreenshot",
            Some(r#"{"format":"png"}"#),
            Some(Internal::Tool(Tool::Screenshot)),
        )?;
        Ok(())
    }

    /// Clears `origin`'s cookies, storage and caches in the page's request context, then loads
    /// the page again (the shipping `clearSiteData`, chromium/shell/siteControls.ts).
    pub fn clear_site_data(&self, tab: &TabId, origin: &str) -> Result<()> {
        if !(origin.starts_with("https://") || origin.starts_with("http://")) {
            bail!("Only a web page's site data can be cleared");
        }
        let (slot, host) = self.tool_slot(tab)?;
        let params = serde_json::json!({ "origin": origin, "storageTypes": "all" }).to_string();
        send(
            slot,
            &host,
            "Storage.clearDataForOrigin",
            Some(&params),
            Some(Internal::Tool(Tool::ClearSiteData {
                origin: origin.to_owned(),
            })),
        )?;
        Ok(())
    }

    /// Copies the image at `url` as the page loads it. The picture arrives as a
    /// [`PageToolEvent::Copied`].
    pub fn copy_image(&self, tab: &TabId, url: &str) -> Result<()> {
        let (slot, host) = self.tool_slot(tab)?;
        let mut callback = CopyImage::new(Rc::downgrade(slot));
        // Not a favicon, no size limit, from the cache when it holds the image.
        host.download_image(Some(&CefString::from(url)), 0, 0, 0, Some(&mut callback));
        Ok(())
    }

    /// Makes the page lay out, take input and identify itself as a phone's, as Chrome's
    /// device mode does, then loads `url` as one (chromium/shell/features/responsive.ts).
    /// The device scale factor stays the window's, so frames keep the pane's size.
    pub fn emulate_phone(&self, tab: &TabId, url: &str) -> Result<()> {
        let (slot, host) = self.tool_slot(tab)?;
        let user_agent = serde_json::json!({
            "userAgent": phone_user_agent(),
            "platform": "Android",
        });
        let metrics = serde_json::json!({
            "width": PHONE_WIDTH,
            // Height 0 keeps the pane's height; scale 0 keeps the window's.
            "height": 0,
            "deviceScaleFactor": 0,
            "mobile": true,
        });
        let calls = [
            ("Emulation.setUserAgentOverride", user_agent),
            ("Emulation.setDeviceMetricsOverride", metrics),
            (
                "Emulation.setTouchEmulationEnabled",
                serde_json::json!({ "enabled": true, "maxTouchPoints": 5 }),
            ),
            (
                "Emulation.setEmitTouchEventsForMouse",
                serde_json::json!({ "enabled": true, "configuration": "mobile" }),
            ),
        ];
        for (method, params) in calls {
            send(
                slot,
                &host,
                method,
                Some(&params.to_string()),
                Some(Internal::Ignore),
            )?;
        }
        // CDP runs calls in order, so the page's first request goes out as the phone's.
        let navigate = serde_json::json!({ "url": url }).to_string();
        send(
            slot,
            &host,
            "Page.navigate",
            Some(&navigate),
            Some(Internal::Ignore),
        )?;
        Ok(())
    }

    /// The next page tool's outcome from a browser the engine still owns.
    pub fn next_page_tool_event(&mut self) -> Option<PageToolMessage> {
        loop {
            let (tab, event) = self.services().page_tool_events.pop()?;
            if let Some(tab) = tab {
                return Some(PageToolMessage { tab, event });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screenshot_reply_decodes_to_its_png_bytes() {
        let png = [0x89, b'P', b'N', b'G', 0, 1, 2];
        let data = base64::engine::general_purpose::STANDARD.encode(png);
        let reply = serde_json::json!({ "data": data }).to_string();
        assert_eq!(screenshot_png(reply.as_bytes()), Some(png.to_vec()));
        assert_eq!(screenshot_png(br#"{"data":"not base64!"}"#), None);
        assert_eq!(screenshot_png(br#"{"other":1}"#), None);
        assert_eq!(screenshot_png(b"not json"), None);
    }

    #[test]
    fn the_phone_user_agent_names_this_chromium() {
        let agent = phone_user_agent();
        assert!(agent.contains(&format!("Chrome/{} Mobile", crate::chromium_version())));
        assert!(agent.starts_with("Mozilla/5.0 (Linux; Android 14; Pixel 8)"));
    }

    #[test]
    fn clearing_site_data_takes_only_a_web_origin() {
        let engine = crate::Engine::new(std::path::PathBuf::new(), || TabId::new("popup"));
        let error = engine
            .clear_site_data(&TabId::new("tab"), "file:///home")
            .unwrap_err();
        assert!(error.to_string().contains("web page"));
    }

    #[test]
    fn clearing_reloads_only_a_page_still_on_the_cleared_origin() {
        assert!(on_origin(
            "https://Example.com:443/a?b#c",
            "https://example.com"
        ));
        assert!(!on_origin("https://other.example/", "https://example.com"));
        assert!(!on_origin("http://example.com/", "https://example.com"));
        assert!(!on_origin("about:blank", "https://example.com"));
    }
}
