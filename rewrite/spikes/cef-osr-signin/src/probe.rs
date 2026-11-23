//! `--probe`: test-only page probes for `scripts/auto-checks.sh`. On a main-frame load whose
//! URL starts with one of the fixed prefixes below, a fixed script reports facts about the
//! page as `[probe] {json}`: which button is present and whether it is enabled and focused,
//! field value lengths, a background colour. It never reports text, field values, cookies or
//! anything a page could put there, and the only input it gives is focus (so the next real key
//! press from xdotool lands in the right element) and, on Google's cookie consent page, a
//! click on "Reject all", as the self-check does. There is no way to run any other script.
//!
//! Reports carry a per-run nonce. Values are accepted only when they are booleans, numbers or
//! short strings from a small alphabet, so even a page that learned the nonce could not use
//! the probe to print its own text.

use crate::common::userfree_to_string;
use cef::*;
use serde_json::{Map, Value};
use std::sync::OnceLock;

const MARKER: &str = "FLECHA_PROBE ";

/// URL prefixes the probe script is injected into. Each is a page of the manual checklist:
/// Google's cookie consent page (the Web Store redirects there from an EU address), Dark
/// Reader's store page, example.com for the content script, and the public herokuapp demo
/// login for password save and fill.
const PAGES: &[&str] = &[
    "https://consent.google.com/",
    "https://chromewebstore.google.com/detail/dark-reader/eimadpbcbfnmbkopoojfekhnkhdbieeh",
    "https://example.com/",
    "https://the-internet.herokuapp.com/login",
    "https://the-internet.herokuapp.com/secure",
];

static NONCE: OnceLock<String> = OnceLock::new();

pub fn enable() {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    NONCE
        .set(format!("p{:x}{:x}", nanos, std::process::id()))
        .expect("probe enabled once");
}

fn nonce() -> Option<&'static str> {
    NONCE.get().map(String::as_str)
}

pub fn on_main_frame_load_end(frame: &mut Frame) {
    let Some(nonce) = nonce() else { return };
    let url = userfree_to_string(&frame.url());
    if !PAGES.iter().any(|p| url.starts_with(p)) {
        return;
    }
    let js = SCRIPT.replace("__NONCE__", nonce);
    frame.execute_java_script(
        Some(&CefString::from(js.as_str())),
        Some(&CefString::from(url.as_str())),
        0,
    );
}

/// Returns true when the message was a probe report (printed or dropped).
pub fn on_console_message(message: &str) -> bool {
    let Some(rest) = message.strip_prefix(MARKER) else {
        return false;
    };
    let Some(nonce) = nonce() else { return true };
    let Some(payload) = rest.strip_prefix(nonce).and_then(|r| r.strip_prefix(' ')) else {
        return true;
    };
    match serde_json::from_str::<Value>(payload) {
        Ok(Value::Object(map)) if allowed(&map) => {
            eprintln!("[probe] {}", Value::Object(map));
        }
        _ => eprintln!("[probe] dropped a report with values outside the allowed forms"),
    }
    true
}

fn allowed(map: &Map<String, Value>) -> bool {
    let short = |s: &str| {
        s.len() <= 32
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || " (),.-_".contains(c))
    };
    map.len() <= 8
        && map.iter().all(|(k, v)| {
            short(k)
                && match v {
                    Value::Bool(_) | Value::Number(_) => true,
                    Value::String(s) => short(s),
                    _ => false,
                }
        })
}

/// Reports whenever the facts change, polling every 500 ms (a test cadence, not a product
/// value). Store: the install button's kind (`add`, `remove` or `none`), whether it is
/// enabled, whether it has focus, its centre in CSS pixels, whether a trusted (real input)
/// click reached it, and whether the page has keyboard focus (a browser dialog over it takes
/// focus); while the page has focus, the probe keeps focus on an enabled "Add to Chrome".
/// example.com: the computed background colours. Login: the lengths of the two fields and
/// which one has focus; the probe focuses the username field once. Secure page: whether the
/// logout link is present.
const SCRIPT: &str = r#"(() => {
  if (window.__flechaProbe) return;
  window.__flechaProbe = true;
  const send = r => console.log('FLECHA_PROBE __NONCE__ ' + JSON.stringify(r));
  let last = '';
  const emit = r => { const s = JSON.stringify(r); if (s !== last) { last = s; send(r); } };
  const label = el => (el.innerText || el.getAttribute('aria-label') || el.value || '').trim();
  const every = f => { const run = () => { try { f(); } catch (e) { emit({ page: 'error' }); } }; run(); setInterval(run, 500); };
  const host = location.hostname, path = location.pathname;
  if (host === 'consent.google.com') {
    every(() => {
      if (window.__flechaConsent) return;
      const b = [...document.querySelectorAll('button, input[type=submit]')]
        .find(x => /^reject all$/i.test(label(x)));
      if (b) { window.__flechaConsent = true; emit({ page: 'consent', rejected: true }); b.click(); }
    });
  } else if (host === 'chromewebstore.google.com') {
    let activated = false;
    const watched = new WeakSet();
    every(() => {
      const b = [...document.querySelectorAll('button')]
        .find(x => /^(add to|remove from) chrome$/i.test(label(x)));
      const r = { page: 'store', button: b ? (/^add/i.test(label(b)) ? 'add' : 'remove') : 'none' };
      if (b) {
        r.enabled = !b.disabled && b.getAttribute('aria-disabled') !== 'true';
        if (!watched.has(b)) {
          watched.add(b);
          b.addEventListener('click', e => { if (e.isTrusted) activated = true; });
        }
        if (r.button === 'add' && r.enabled && document.hasFocus() && document.activeElement !== b) b.focus();
        r.focused = document.activeElement === b;
        const box = b.getBoundingClientRect();
        r.x = Math.round(box.left + box.width / 2);
        r.y = Math.round(box.top + box.height / 2);
      }
      r.activated = activated;
      r.page_focus = document.hasFocus();
      emit(r);
    });
  } else if (host === 'example.com') {
    every(() => emit({
      page: 'example',
      body: getComputedStyle(document.body).backgroundColor,
      html: getComputedStyle(document.documentElement).backgroundColor,
    }));
  } else if (host === 'the-internet.herokuapp.com' && path === '/login') {
    let focused = false;
    every(() => {
      const u = document.getElementById('username'), p = document.getElementById('password');
      if (u && !focused) { focused = true; u.focus(); }
      const a = document.activeElement;
      emit({ page: 'login', user_len: u ? u.value.length : -1, pass_len: p ? p.value.length : -1,
             focus: a === u ? 'username' : a === p ? 'password' : 'other' });
    });
  } else if (host === 'the-internet.herokuapp.com' && path === '/secure') {
    every(() => emit({ page: 'secure', logged_in: !!document.querySelector('a[href="/logout"]') }));
  }
})();"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_carry_only_flags_numbers_and_short_plain_strings() {
        let ok: Value = serde_json::json!({ "page": "store", "button": "add", "enabled": true,
            "body": "rgb(24, 26, 27)", "user_len": 8 });
        assert!(allowed(ok.as_object().unwrap()));
        for bad in [
            serde_json::json!({ "page": "Inbox (3) – someone@example.com" }),
            serde_json::json!({ "text": "SuperSecretPassword!" }),
            serde_json::json!({ "nested": { "a": 1 } }),
            serde_json::json!({ "list": [1] }),
            serde_json::json!({ "page": "x".repeat(33) }),
        ] {
            assert!(!allowed(bad.as_object().unwrap()), "{bad}");
        }
    }
}
