//! `--mode chrome`: the control. A Chrome-style browser created with
//! `CefBrowserHost::CreateBrowser` and no parent window, so CEF opens a normal Chrome browser
//! window with Chrome's own tab strip and toolbar. The extension menu, password bubble and
//! permission prompts have their usual anchors there.

use crate::common::{self, SpikeDisplayHandler, SpikeLoadHandler};
use cef::*;

wrap_client! {
    pub struct ChromeClient;

    impl Client {
        fn display_handler(&self) -> Option<DisplayHandler> {
            Some(SpikeDisplayHandler::new(None))
        }

        fn load_handler(&self) -> Option<LoadHandler> {
            Some(SpikeLoadHandler::new())
        }

        fn life_span_handler(&self) -> Option<LifeSpanHandler> {
            Some(ChromeLifeSpanHandler::new())
        }
    }
}

wrap_life_span_handler! {
    struct ChromeLifeSpanHandler;

    impl LifeSpanHandler {
        fn on_after_created(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                common::browser_created(browser);
            }
        }

        fn on_before_close(&self, browser: Option<&mut Browser>) {
            if let Some(browser) = browser {
                common::browser_closed(browser);
            }
        }
    }
}

/// Called from `on_context_initialized`.
pub fn create_browser(url: &str) {
    let window_info = WindowInfo {
        runtime_style: RuntimeStyle::CHROME,
        ..Default::default()
    };
    let mut client = ChromeClient::new();
    let created = browser_host_create_browser(
        Some(&window_info),
        Some(&mut client),
        Some(&CefString::from(url)),
        Some(&common::browser_settings()),
        None,
        None,
    );
    if created != 1 {
        common::fail_startup("browser_host_create_browser failed");
    }
}
