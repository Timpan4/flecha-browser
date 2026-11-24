use std::rc::Rc;

use cef::{rc::Rc as _, *};
use flecha_core::TabId;

use crate::callbacks::Slot;
use crate::{BrowserKey, Engine};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FindResult {
    pub count: i32,
    pub active_match_ordinal: i32,
    pub final_update: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct FindEvent {
    pub key: BrowserKey,
    pub result: FindResult,
}

wrap_find_handler! {
    struct Find { slot: Rc<Slot> }
    impl FindHandler {
        fn on_find_result(
            &self, _browser: Option<&mut Browser>, _identifier: i32, count: i32,
            _selection: Option<&Rect>, active_match_ordinal: i32, final_update: i32,
        ) {
            if !self.slot.finding.get() || self.slot.retired.get() {
                return;
            }
            self.slot.popups.services.find_events.borrow_mut().push_back(FindEvent {
                key: self.slot.key.borrow().clone(),
                result: FindResult { count, active_match_ordinal, final_update: final_update != 0 },
            });
        }
    }
}

pub(crate) fn handler(slot: Rc<Slot>) -> FindHandler {
    Find::new(slot)
}

pub(crate) fn forget(slot: &Slot) {
    let key = slot.key.borrow();
    slot.popups
        .services
        .find_events
        .borrow_mut()
        .retain(|event| event.key != *key);
}

pub(crate) fn stop(slot: &Slot, clear_selection: bool) {
    slot.finding.set(false);
    forget(slot);
    let host = slot.browser.borrow().as_ref().and_then(ImplBrowser::host);
    if let Some(host) = host {
        host.stop_finding(i32::from(clear_selection));
    }
}

impl Engine {
    pub fn find(&self, tab: &TabId, query: &str, forward: bool, match_case: bool, find_next: bool) {
        if query.is_empty() {
            self.stop_finding(tab, true);
            return;
        }
        if let (Some(session), Some(host)) = (self.browsers.get(tab), self.host(tab)) {
            if !find_next {
                // CEF discards replies from earlier search sessions, but some may already be queued here.
                forget(&session.slot);
            }
            session.slot.finding.set(true);
            host.find(
                Some(&CefString::from(query)),
                i32::from(forward),
                i32::from(match_case),
                i32::from(find_next),
            );
        }
    }

    pub fn stop_finding(&self, tab: &TabId, clear_selection: bool) {
        if let Some(session) = self.browsers.get(tab) {
            stop(&session.slot, clear_selection);
        }
    }

    pub fn next_find_event(&mut self) -> Option<FindEvent> {
        self.services().find_events.borrow_mut().pop_front()
    }
}
