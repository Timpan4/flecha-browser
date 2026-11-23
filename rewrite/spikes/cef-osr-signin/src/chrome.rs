//! `--mode chrome`: the control. A Chrome-style browser in a CEF Views window with Chrome's
//! normal toolbar, so the extension menu, password bubble and permission prompts have their
//! usual anchors. Based on cef-rs examples/cefsimple.

use crate::common::{self, SpikeDisplayHandler, SpikeLoadHandler};
use cef::*;
use std::cell::RefCell;

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

wrap_browser_view_delegate! {
    struct ChromeBrowserViewDelegate {}

    impl ViewDelegate {}

    impl BrowserViewDelegate {
        fn chrome_toolbar_type(&self, _browser_view: Option<&mut BrowserView>) -> ChromeToolbarType {
            ChromeToolbarType::NORMAL
        }

        fn browser_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::CHROME
        }
    }
}

wrap_window_delegate! {
    struct ChromeWindowDelegate {
        browser_view: RefCell<Option<BrowserView>>,
    }

    impl ViewDelegate {
        fn preferred_size(&self, _view: Option<&mut View>) -> Size {
            // cefsimple's window size.
            Size { width: 800, height: 600 }
        }
    }

    impl PanelDelegate {}

    impl WindowDelegate {
        fn on_window_created(&self, window: Option<&mut Window>) {
            let browser_view = self.browser_view.borrow();
            let (Some(window), Some(browser_view)) = (window, browser_view.as_ref()) else {
                return;
            };
            // Toolbar above the page, as cefclient's ViewsWindow lays it out. CEF only makes
            // the toolbar available (GetChromeToolbar) once the browser view is in a window.
            let layout = window.set_to_box_layout(Some(&BoxLayoutSettings {
                horizontal: 0,
                cross_axis_alignment: AxisAlignment::STRETCH,
                ..Default::default()
            }));
            let mut view = View::from(browser_view);
            window.add_child_view(Some(&mut view));
            if let Some(mut toolbar) = browser_view.chrome_toolbar() {
                window.add_child_view_at(Some(&mut toolbar), 0);
            } else {
                eprintln!("[chrome] no Chrome toolbar available");
            }
            if let Some(layout) = layout {
                layout.set_flex_for_view(Some(&mut view), 1);
            }
            window.layout();
            window.show();
        }

        fn on_window_destroyed(&self, _window: Option<&mut Window>) {
            *self.browser_view.borrow_mut() = None;
        }

        fn can_close(&self, _window: Option<&mut Window>) -> i32 {
            let browser_view = self.browser_view.borrow();
            match browser_view.as_ref().and_then(|v| v.browser()).and_then(|b| b.host()) {
                Some(host) => host.try_close_browser(),
                None => 1,
            }
        }

        fn window_runtime_style(&self) -> RuntimeStyle {
            RuntimeStyle::CHROME
        }
    }
}

/// Called from `on_context_initialized`.
pub fn create_browser(url: &str) {
    let mut client = ChromeClient::new();
    let mut delegate = ChromeBrowserViewDelegate::new();
    let settings = common::browser_settings();
    let browser_view = browser_view_create(
        Some(&mut client),
        Some(&CefString::from(url)),
        Some(&settings),
        None,
        None,
        Some(&mut delegate),
    );
    let mut window_delegate = ChromeWindowDelegate::new(RefCell::new(browser_view));
    window_create_top_level(Some(&mut window_delegate));
}
