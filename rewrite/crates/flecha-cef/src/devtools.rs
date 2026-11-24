//! The in-process Chrome DevTools Protocol bridge.
//!
//! Messages go through `CefBrowserHost::SendDevToolsMessage` and replies come back through a
//! `DevToolsMessageObserver` registered per browser, as Spike 2 proved: no remote-debugging port
//! or pipe is opened, and `navigator.webdriver` stays `false`.

use std::rc::Weak;

use anyhow::{Context as _, Result, ensure};
use cef::{rc::Rc as _, *};
use serde_json::{Map, Value};

use crate::callbacks::Slot;
use crate::services;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DevToolsEvent {
    /// The reply to [`crate::Engine::send_devtools_message`]'s message `id`. `result` is the
    /// JSON `result` object on success and the `error` object on failure.
    Result {
        id: i32,
        success: bool,
        result: String,
    },
    /// A CDP event, such as `Page.loadEventFired` after `Page.enable`.
    Event { method: String, params: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevToolsMessage {
    pub tab: flecha_core::TabId,
    pub event: DevToolsEvent,
}

impl crate::Engine {
    /// Sends one CDP method call to the tab's page and returns its message id. The reply
    /// arrives as a [`DevToolsEvent::Result`] with that id from [`Self::next_devtools_event`].
    /// `params_json`, when given, must be a JSON object.
    pub fn send_devtools_message(
        &self,
        tab: &flecha_core::TabId,
        method: &str,
        params_json: Option<&str>,
    ) -> Result<i32> {
        let session = self.browsers.get(tab).context("no browser for this tab")?;
        let host = self.host(tab).context("the tab's browser isn't created yet")?;
        send(&session.slot, &host, method, params_json, false)
    }

    /// The next CDP reply or event from a browser the engine still owns.
    pub fn next_devtools_event(&mut self) -> Option<DevToolsMessage> {
        self.adopt_popups();
        loop {
            let (tab, event) = self.services().devtools_events.pop()?;
            if let Some(tab) = tab {
                return Some(DevToolsMessage { tab, event });
            }
        }
    }

    /// Opens Chromium's DevTools for the tab in a window of its own.
    pub fn open_devtools(&self, tab: &flecha_core::TabId) -> bool {
        let Some(host) = self.host(tab) else {
            return false;
        };
        host.show_dev_tools(
            Some(&WindowInfo::default()),
            None,
            Some(&BrowserSettings::default()),
            None,
        );
        true
    }

    pub fn close_devtools(&self, tab: &flecha_core::TabId) {
        if let Some(host) = self.host(tab) {
            host.close_dev_tools();
        }
    }
}

/// The JSON text of one CDP method call. `params`, when given, must be a JSON object.
pub(crate) fn frame(id: i32, method: &str, params: Option<&str>) -> Result<Vec<u8>> {
    ensure!(id > 0, "CDP message ids are positive");
    ensure!(
        !method.is_empty() && method.contains('.'),
        "a CDP method is named Domain.method"
    );
    let mut message = Map::new();
    message.insert("id".into(), id.into());
    message.insert("method".into(), method.into());
    if let Some(params) = params {
        let params: Value = serde_json::from_str(params)?;
        ensure!(params.is_object(), "CDP params must be a JSON object");
        message.insert("params".into(), params);
    }
    Ok(serde_json::to_vec(&Value::Object(message))?)
}

fn utf8(bytes: Option<&[u8]>) -> String {
    String::from_utf8_lossy(bytes.unwrap_or_default()).into_owned()
}

wrap_dev_tools_message_observer! {
    pub(crate) struct Observer { slot: Weak<Slot> }
    impl DevToolsMessageObserver {
        fn on_dev_tools_method_result(
            &self, _browser: Option<&mut Browser>, message_id: ::std::os::raw::c_int,
            success: ::std::os::raw::c_int, result: Option<&[u8]>,
        ) {
            let Some(slot) = self.slot.upgrade() else { return };
            let services = services::of(&slot);
            if services.devtools_internal.borrow_mut().remove(&message_id) {
                return;
            }
            services.devtools_events.push(self.slot.clone(), DevToolsEvent::Result {
                id: message_id,
                success: success != 0,
                result: utf8(result),
            });
        }

        fn on_dev_tools_event(
            &self, _browser: Option<&mut Browser>, method: Option<&CefString>, params: Option<&[u8]>,
        ) {
            let Some(slot) = self.slot.upgrade() else { return };
            services::of(&slot).devtools_events.push(self.slot.clone(), DevToolsEvent::Event {
                method: method.map(ToString::to_string).unwrap_or_default(),
                params: utf8(params),
            });
        }
    }
}

/// Sends one CDP call to `slot`'s browser and returns its message id. The first call registers
/// the observer that reports replies; `on_before_close` drops it.
pub(crate) fn send(
    slot: &std::rc::Rc<Slot>,
    host: &BrowserHost,
    method: &str,
    params: Option<&str>,
    internal: bool,
) -> Result<i32> {
    let services = services::of(slot);
    if slot.devtools.borrow().is_none() {
        let mut observer = Observer::new(std::rc::Rc::downgrade(slot));
        let registration = host.add_dev_tools_message_observer(Some(&mut observer));
        ensure!(registration.is_some(), "CEF refused the DevTools observer");
        slot.devtools.replace(registration);
    }
    let id = services.next_devtools_id();
    let message = frame(id, method, params)?;
    if internal {
        services.devtools_internal.borrow_mut().insert(id);
    }
    let sent = host.send_dev_tools_message(Some(&message)) != 0;
    if !sent {
        services.devtools_internal.borrow_mut().remove(&id);
    }
    ensure!(sent, "CEF did not accept the DevTools message");
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cdp_calls_frame_as_one_json_object_with_id_method_and_params() {
        let bytes = frame(7, "Runtime.evaluate", Some(r#"{"expression":"navigator.webdriver"}"#))
            .unwrap();
        let message: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            message,
            serde_json::json!({
                "id": 7,
                "method": "Runtime.evaluate",
                "params": { "expression": "navigator.webdriver" },
            })
        );

        let bare: Value = serde_json::from_slice(&frame(1, "Page.enable", None).unwrap()).unwrap();
        assert_eq!(bare, serde_json::json!({ "id": 1, "method": "Page.enable" }));
    }

    #[test]
    fn malformed_calls_never_reach_cef() {
        // CEF drops malformed messages without a reply, so the caller would wait forever.
        assert!(frame(1, "Runtime.evaluate", Some("[1, 2]")).is_err());
        assert!(frame(1, "Runtime.evaluate", Some("{not json")).is_err());
        assert!(frame(1, "", None).is_err());
        assert!(frame(1, "evaluate", None).is_err());
        assert!(frame(0, "Page.enable", None).is_err());
        // A method name can't smuggle in another field.
        let tricky: Value =
            serde_json::from_slice(&frame(2, "A.b\",\"id\":9", None).unwrap()).unwrap();
        assert_eq!(tricky["id"], 2);
    }

    #[test]
    fn message_ids_stay_positive_when_the_counter_wraps() {
        let services = crate::services::Services::new();
        services.devtools_next.set(i32::MAX);
        assert_eq!(services.next_devtools_id(), i32::MAX);
        assert_eq!(services.next_devtools_id(), 1);
    }
}
