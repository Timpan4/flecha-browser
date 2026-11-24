//! Boosts styles and scripts on web pages, from flecha-boosts' store.
//!
//! Styles go in as constructed style sheets on `document.adoptedStyleSheets`, which a page's
//! `style-src` CSP doesn't govern. They're applied when the main frame commits and again when it
//! finishes loading, and each application replaces the previous one, so it is idempotent. Scripts
//! run once the main frame has loaded, each as its own classic script in the page's world;
//! flecha-boosts wraps each to run once per document.

use cef::{ImplBrowser as _, ImplFrame as _, *};
use flecha_boosts::{Boosts, Effect, Frame as BoostFrame, ScriptInjection, StyleInjection};
use serde_json::json;

use crate::services::Services;

/// The page registry that holds the style Boosts' adopted style sheets.
pub(crate) const BOOST_SHEETS: &str = "flecha.boost/styles";

/// Whether Boosts and cosmetic filters may touch the page: flecha-boosts' web-only rule.
pub(crate) fn is_web_page(url: &str) -> bool {
    url::Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

/// Script that makes `registry`'s sheets on the page exactly `sheets` (name, CSS), in order.
/// Sheets from earlier applications that `sheets` no longer names are removed.
pub(crate) fn style_payload(registry: &str, sheets: &[(String, String)]) -> String {
    let registry = json!(registry);
    let sheets = json!(sheets);
    format!(
        r#"(() => {{
const key = Symbol.for({registry});
let sheets = globalThis[key];
if (!(sheets instanceof Map)) {{
  sheets = new Map();
  Object.defineProperty(globalThis, key, {{ value: sheets }});
}}
const want = new Map({sheets});
const drop = [];
for (const [name, sheet] of sheets) if (!want.has(name)) {{ drop.push(sheet); sheets.delete(name); }}
let adopted = document.adoptedStyleSheets.filter(sheet => !drop.includes(sheet));
for (const [name, css] of want) {{
  let sheet = sheets.get(name);
  if (!sheet) {{ sheet = new CSSStyleSheet(); sheets.set(name, sheet); adopted = [...adopted, sheet]; }}
  sheet.replaceSync(css);
}}
document.adoptedStyleSheets = adopted;
}})();"#
    )
}

/// The style Boosts for a main-frame document at `url`, as a payload. `None` off the web.
pub(crate) fn styles_for(boosts: &Boosts, url: &str) -> Option<String> {
    if !is_web_page(url) {
        return None;
    }
    let sheets: Vec<(String, String)> = boosts
        .styles_for(BoostFrame::Main, url)
        .into_iter()
        .map(|StyleInjection { name, css }| (name.as_str().to_owned(), css))
        .collect();
    Some(style_payload(BOOST_SHEETS, &sheets))
}

/// The script Boosts for a loaded main-frame document at `url`: (script URL, source) pairs.
pub(crate) fn scripts_for(boosts: &Boosts, url: &str) -> Vec<(String, String)> {
    if !is_web_page(url) {
        return Vec::new();
    }
    boosts
        .scripts_for(BoostFrame::Main, url)
        .iter()
        .map(|script: &ScriptInjection| {
            (format!("flecha-boost/{}", script.name()), script.source())
        })
        .collect()
}

impl crate::Engine {
    /// Replaces the Boosts pages get from their next load. Call [`Self::reload_boosts`] or
    /// [`Self::apply_boost_effect`] to update open pages.
    pub fn set_boosts(&self, boosts: Boosts) {
        self.services().boosts.replace(boosts);
    }

    pub fn boosts(&self) -> std::cell::Ref<'_, Boosts> {
        self.services().boosts.borrow()
    }

    /// Edits the Boosts and applies the edit's [`Effect`] to open pages.
    pub fn edit_boosts<R>(&self, edit: impl FnOnce(&mut Boosts) -> (R, Effect)) -> R {
        let (result, effect) = edit(&mut self.services().boosts.borrow_mut());
        self.apply_boost_effect(&effect);
        result
    }

    /// Restyles or reloads the open pages a saved Boost affects.
    pub fn apply_boost_effect(&self, effect: &Effect) {
        for (tab, session) in &self.browsers {
            let Some(url) = self.url(tab) else { continue };
            if !effect.affects(&url) {
                continue;
            }
            let browser = session.slot.browser.borrow().clone();
            match effect {
                Effect::Reload { .. } => {
                    if let Some(browser) = browser {
                        browser.reload();
                    }
                }
                Effect::Restyle { .. } => {
                    if let Some(frame) = browser.and_then(|browser| browser.main_frame()) {
                        page_committed(self.services(), &frame, &url);
                    }
                }
                Effect::Saved | Effect::Unchanged => {}
            }
        }
    }

    /// Re-applies the style Boosts on every open web page, adding, changing and removing sheets
    /// to match the current Boosts.
    pub fn reload_boosts(&self) {
        for tab in self.browsers.keys() {
            let Some(url) = self.url(tab) else { continue };
            let frame = self.browser(tab).and_then(|browser| browser.main_frame());
            if let Some(frame) = frame {
                page_committed(self.services(), &frame, &url);
            }
        }
    }
}

/// The main frame committed `url`: its style Boosts apply before it parses.
pub(crate) fn page_committed(services: &Services, frame: &Frame, url: &str) {
    let styles = styles_for(&services.boosts.borrow(), url);
    if let Some(styles) = styles {
        run(frame, "flecha-boost/styles", &styles);
    }
}

/// The main frame finished loading `url`: styles again, in case the document replaced what the
/// commit applied, then script Boosts.
pub(crate) fn page_loaded(services: &Services, frame: &Frame, url: &str) {
    if !is_web_page(url) {
        return;
    }
    let (styles, scripts) = {
        let boosts = services.boosts.borrow();
        (styles_for(&boosts, url), scripts_for(&boosts, url))
    };
    if let Some(styles) = styles {
        run(frame, "flecha-boost/styles", &styles);
    }
    for (script_url, source) in scripts {
        run(frame, &script_url, &source);
    }
}

pub(crate) fn run(frame: &Frame, script_url: &str, source: &str) {
    frame.execute_java_script(
        Some(&CefString::from(source)),
        Some(&CefString::from(script_url)),
        1,
    );
}

#[cfg(test)]
mod tests {
    use flecha_boosts::{BoostKind, CompiledBoost};

    use super::*;

    fn store() -> Boosts {
        let mut boosts = Boosts::new();
        boosts
            .save("example.com.css", "body { color: red }", None, false)
            .unwrap();
        boosts
            .save(
                "example.com.ts",
                "document.body.dataset.boosted = '1';",
                Some(CompiledBoost {
                    code: Some("document.body.dataset.boosted = '1';".into()),
                    error: None,
                }),
                false,
            )
            .unwrap();
        boosts
    }

    #[test]
    fn web_pages_get_their_sites_styles_and_scripts() {
        let boosts = store();
        let styles = styles_for(&boosts, "https://www.example.com/page").unwrap();
        assert!(styles.contains(r#"Symbol.for("flecha.boost/styles")"#));
        assert!(
            styles.contains(r#"[["example.com.css","body { color: red }"]]"#),
            "{styles}"
        );

        let scripts = scripts_for(&boosts, "https://www.example.com/page");
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].0, "flecha-boost/example.com.ts");
        assert!(scripts[0].1.contains("document.body.dataset.boosted"));
        assert!(
            scripts[0].1.contains("Object.hasOwn(globalThis"),
            "runs once per document"
        );

        // Another site's page gets an empty sheet set, which removes stale sheets.
        let other = styles_for(&boosts, "https://other.test/").unwrap();
        assert!(other.contains("new Map([])"), "{other}");
        assert!(scripts_for(&boosts, "https://other.test/").is_empty());
    }

    #[test]
    fn boosts_never_touch_non_web_pages() {
        let mut boosts = store();
        boosts.create("all", BoostKind::Styles).unwrap();
        for url in [
            "chrome://settings/",
            "file:///home/user/example.com/index.html",
            "about:blank",
            "data:text/html,<p>example.com</p>",
            "devtools://devtools/bundled/inspector.html",
        ] {
            assert_eq!(styles_for(&boosts, url), None, "{url}");
            assert!(scripts_for(&boosts, url).is_empty(), "{url}");
        }
    }

    #[test]
    fn css_is_embedded_as_a_json_string_so_it_cannot_end_the_script() {
        let hostile = "a{} \"); alert(1); (\" </script> \u{2028}";
        let payload = style_payload(BOOST_SHEETS, &[("x.css".into(), hostile.into())]);
        let encoded = serde_json::to_string(hostile).unwrap();
        assert!(payload.contains(&encoded), "{payload}");
        assert!(!payload.contains("\"); alert(1); (\""));
    }
}
