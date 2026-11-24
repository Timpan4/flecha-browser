//! CEF's permission requests, decided by flecha-permissions.
//!
//! Alloy-style OSR browsers show no permission UI, so every `getUserMedia` request and every
//! permission prompt comes here. A request is answered at once from saved and one-time decisions,
//! or held until the shell answers the tab's prompt banner. Anything this module can't vouch for
//! is denied: an opaque, invalid or insecure requesting origin, a frame whose document isn't the
//! requesting origin, a page that isn't a secure context, a request kind flecha-permissions
//! doesn't know, and every request still held when its tab navigates or closes.
//!
//! Requests are queued per CEF browser rather than per tab id, so a prompt follows its browser
//! when the shell moves it to another tab.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Weak;

use cef::{rc::Rc as _, *};
use flecha_core::TabId;
use flecha_permissions::{
    Completion, Evaluation, Origin, Partition, PermissionError, PermissionKind, PermissionManager,
    PermissionSet, PromptOutcome, RequestId, TabId as QueueId, Verdict,
};

use crate::callbacks::Slot;
use crate::services::{self, Services};

/// The front of a tab's prompt queue, for the shell's banner.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionPrompt {
    /// Pass to [`crate::Engine::resolve_permission`].
    pub request: RequestId,
    pub origin: Origin,
    /// Every kind the page asked for.
    pub requested: PermissionSet,
    /// The kinds the banner asks about.
    pub ask: PermissionSet,
}

/// The banner a tab shows changed: a new prompt reached the front of its queue, or `None` when
/// the queue emptied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermissionEvent {
    pub tab: TabId,
    pub prompt: Option<PermissionPrompt>,
}

/// How a held request is answered. CEF's callbacks in the app, recorders in tests.
pub(crate) trait Answer {
    fn answer(self: Box<Self>, verdict: Verdict);
}

struct Held {
    answer: Box<dyn Answer>,
    queue: QueueId,
    /// CEF's prompt id, for `OnDismissPermissionPrompt`. Media requests have none.
    prompt: Option<u64>,
}

#[derive(Default)]
pub(crate) struct Permissions {
    pub manager: PermissionManager,
    held: BTreeMap<RequestId, Held>,
    prompts: BTreeMap<u64, RequestId>,
    /// The browser behind each queue, for routing banner events.
    browsers: BTreeMap<QueueId, Weak<Slot>>,
    /// The request each queue's banner shows.
    shown: BTreeMap<QueueId, RequestId>,
}

type Ready = Vec<(Box<dyn Answer>, Verdict)>;

impl Permissions {
    fn complete(&mut self, completions: Vec<Completion>, touched: &mut BTreeSet<QueueId>) -> Ready {
        completions
            .into_iter()
            .filter_map(|Completion { request, verdict }| {
                let held = self.held.remove(&request)?;
                if let Some(prompt) = held.prompt {
                    self.prompts.remove(&prompt);
                }
                touched.insert(held.queue);
                Some((held.answer, verdict))
            })
            .collect()
    }

    /// Reports each touched queue whose front prompt changed.
    fn refresh(&mut self, touched: BTreeSet<QueueId>, events: &services::Mailbox<Option<PermissionPrompt>>) {
        for queue in touched {
            let front = self.manager.pending_prompt(&queue).map(|view| PermissionPrompt {
                request: view.request,
                origin: view.origin.clone(),
                requested: view.requested,
                ask: view.ask,
            });
            let front_id = front.as_ref().map(|prompt| prompt.request);
            if self.shown.get(&queue).copied() == front_id {
                continue;
            }
            let browser = self.browsers.get(&queue).cloned().unwrap_or_default();
            match front_id {
                Some(id) => {
                    self.shown.insert(queue, id);
                }
                None => {
                    self.shown.remove(&queue);
                    self.browsers.remove(&queue);
                }
            }
            events.push(browser, front);
        }
    }

    pub fn pending(&self, queue: &QueueId) -> Option<PermissionPrompt> {
        self.manager.pending_prompt(queue).map(|view| PermissionPrompt {
            request: view.request,
            origin: view.origin.clone(),
            requested: view.requested,
            ask: view.ask,
        })
    }
}

impl crate::Engine {
    /// The next change to a tab's permission banner.
    pub fn next_permission_event(&mut self) -> Option<PermissionEvent> {
        self.adopt_popups();
        loop {
            let (tab, prompt) = self.services().permission_events.pop()?;
            if let Some(tab) = tab {
                return Some(PermissionEvent { tab, prompt });
            }
        }
    }

    /// The prompt the tab's banner shows now.
    pub fn pending_permission_prompt(&self, tab: &TabId) -> Option<PermissionPrompt> {
        let identifier = self.browsers.get(tab)?.slot.identifier.get()?;
        self.services().permissions.borrow().pending(&queue_id(identifier))
    }

    /// Answers the banner's prompt and every request the answer decides.
    pub fn resolve_permission(
        &mut self,
        request: RequestId,
        outcome: PromptOutcome,
    ) -> Result<(), PermissionError> {
        resolve(self.services(), request, outcome)
    }

    /// Saved decisions, for the site panel and for saving (`PermissionManager::snapshot`).
    pub fn permissions(&self) -> std::cell::Ref<'_, PermissionManager> {
        std::cell::Ref::map(self.services().permissions.borrow(), |state| &state.manager)
    }

    /// Changes decisions, as the site panel does. `change` returns its result and the
    /// manager's completions, which the engine answers:
    /// `engine.permissions_mut(|m| match m.set_setting(..) { Ok(done) => (Ok(()), done), Err(e) => (Err(e), vec![]) })`.
    pub fn permissions_mut<R>(
        &mut self,
        change: impl FnOnce(&mut PermissionManager) -> (R, Vec<Completion>),
    ) -> R {
        apply(self.services(), change)
    }

    /// The flecha-permissions partition of the tab's request context.
    pub fn permission_partition(&self, tab: &TabId) -> Option<Partition> {
        self.browsers
            .get(tab)
            .map(|session| partition(&session.partition))
    }
}

/// The permission queue of the CEF browser with this identifier.
pub(crate) fn queue_id(browser: i32) -> QueueId {
    QueueId::new(format!("browser-{browser}"))
}

pub(crate) fn partition(partition: &crate::Partition) -> Partition {
    match partition {
        crate::Partition::Shared => Partition::Shared,
        crate::Partition::Isolated(space) => Partition::Isolated(space.as_str().to_owned()),
    }
}

/// The origin a request may be decided for, or `None` to deny it (issue #122). The requesting
/// origin must be a secure tuple origin; the page must be a secure context, so its top-level
/// document must be too; and the requesting frame's document, when CEF names it, must be that
/// origin. A sandboxed frame's origin is opaque even when its URL is `https:`, so it fails the
/// first or the last check.
pub(crate) fn eligible_origin(requesting: &str, frame_url: Option<&str>, top_url: &str) -> Option<Origin> {
    let origin = Origin::parse(requesting).ok()?;
    let top = Origin::parse(top_url).ok()?;
    if !origin.is_potentially_trustworthy() || !top.is_potentially_trustworthy() {
        return None;
    }
    if let Some(frame_url) = frame_url
        && Origin::parse(frame_url).ok()? != origin
    {
        return None;
    }
    Some(origin)
}

const MEDIA_AUDIO: u32 =
    sys::cef_media_access_permission_types_t::CEF_MEDIA_PERMISSION_DEVICE_AUDIO_CAPTURE as u32;
const MEDIA_VIDEO: u32 =
    sys::cef_media_access_permission_types_t::CEF_MEDIA_PERMISSION_DEVICE_VIDEO_CAPTURE as u32;

/// The kinds of a `getUserMedia` request. Desktop capture has no kind, so it is denied.
pub(crate) fn media_kinds(bits: u32) -> Option<PermissionSet> {
    kinds(bits, &[(MEDIA_AUDIO, PermissionKind::Microphone), (MEDIA_VIDEO, PermissionKind::Camera)])
}

/// The kinds of a permission prompt. A request with any other bit is denied whole.
pub(crate) fn prompt_kinds(bits: u32) -> Option<PermissionSet> {
    use sys::cef_permission_request_types_t as T;
    kinds(
        bits,
        &[
            (T::CEF_PERMISSION_TYPE_GEOLOCATION as u32, PermissionKind::Geolocation),
            (T::CEF_PERMISSION_TYPE_NOTIFICATIONS as u32, PermissionKind::Notifications),
            (T::CEF_PERMISSION_TYPE_CLIPBOARD as u32, PermissionKind::ClipboardRead),
            (T::CEF_PERMISSION_TYPE_CAMERA_STREAM as u32, PermissionKind::Camera),
            (T::CEF_PERMISSION_TYPE_MIC_STREAM as u32, PermissionKind::Microphone),
        ],
    )
}

fn kinds(bits: u32, known: &[(u32, PermissionKind)]) -> Option<PermissionSet> {
    let mut rest = bits;
    let mut set = PermissionSet::EMPTY;
    for &(bit, kind) in known {
        if bits & bit != 0 {
            set.insert(kind);
            rest &= !bit;
        }
    }
    (rest == 0 && !set.is_empty()).then_some(set)
}

/// One request from a browser, decided now or queued. `origin` and `kinds` are `None` when the
/// request is ineligible, which denies it.
pub(crate) struct Ask {
    pub browser: Weak<Slot>,
    pub queue: QueueId,
    pub partition: Option<Partition>,
    pub origin: Option<Origin>,
    pub kinds: Option<PermissionSet>,
    pub prompt: Option<u64>,
}

pub(crate) fn request(services: &Services, ask: Ask, answer: Box<dyn Answer>) {
    let (Some(partition), Some(origin), Some(kinds)) = (ask.partition, ask.origin, ask.kinds) else {
        answer.answer(Verdict::Block);
        return;
    };
    let now = {
        let mut state = services.permissions.borrow_mut();
        match state.manager.evaluate(&partition, &ask.queue, &origin, kinds) {
            Evaluation::Resolved(verdict) => Some((answer, verdict)),
            Evaluation::Prompt(request) => {
                state.held.insert(
                    request,
                    Held {
                        answer,
                        queue: ask.queue.clone(),
                        prompt: ask.prompt,
                    },
                );
                if let Some(prompt) = ask.prompt {
                    state.prompts.insert(prompt, request);
                }
                state.browsers.insert(ask.queue.clone(), ask.browser);
                state.refresh(BTreeSet::from([ask.queue]), &services.permission_events);
                None
            }
        }
    };
    if let Some((answer, verdict)) = now {
        answer.answer(verdict);
    }
}

/// Applies a manager change and answers every request it completed, after the borrow ends.
pub(crate) fn apply<R>(
    services: &Services,
    change: impl FnOnce(&mut PermissionManager) -> (R, Vec<Completion>),
) -> R {
    let (result, ready) = {
        let mut state = services.permissions.borrow_mut();
        let (result, completions) = change(&mut state.manager);
        let mut touched = BTreeSet::new();
        let ready = state.complete(completions, &mut touched);
        state.refresh(touched, &services.permission_events);
        (result, ready)
    };
    for (answer, verdict) in ready {
        answer.answer(verdict);
    }
    result
}

pub(crate) fn resolve(
    services: &Services,
    request: RequestId,
    outcome: PromptOutcome,
) -> Result<(), PermissionError> {
    apply(services, |manager| match manager.resolve_prompt(request, outcome) {
        Ok(completions) => (Ok(()), completions),
        Err(error) => (Err(error), Vec::new()),
    })
}

/// The tab's page went away (navigation) or its browser closed: deny everything it holds and
/// forget its one-time answers.
pub(crate) fn leave(services: &Services, queue: &QueueId) {
    apply(services, |manager| ((), manager.cancel_tab_prompts(queue)));
}

/// CEF dismissed a prompt itself; its callback is no longer valid, so it is dropped unanswered.
/// A prompt the engine already answered is unknown here, which makes this a no-op.
pub(crate) fn dismissed(services: &Services, prompt: u64) {
    let dropped = {
        let mut state = services.permissions.borrow_mut();
        let Some(request) = state.prompts.remove(&prompt) else {
            return;
        };
        state.manager.withdraw(request);
        let held = state.held.remove(&request);
        if let Some(held) = &held {
            state.refresh(BTreeSet::from([held.queue.clone()]), &services.permission_events);
        }
        held
    };
    drop(dropped);
}

struct MediaAnswer {
    callback: MediaAccessCallback,
    requested: u32,
}

impl Answer for MediaAnswer {
    fn answer(self: Box<Self>, verdict: Verdict) {
        // A getUserMedia grant must match what was requested, so it is all or nothing.
        match verdict {
            Verdict::Allow => self.callback.cont(self.requested),
            Verdict::Block => self.callback.cancel(),
        }
    }
}

struct PromptAnswer {
    callback: PermissionPromptCallback,
}

impl Answer for PromptAnswer {
    fn answer(self: Box<Self>, verdict: Verdict) {
        self.callback.cont(match verdict {
            Verdict::Allow => PermissionRequestResult::ACCEPT,
            Verdict::Block => PermissionRequestResult::DENY,
        });
    }
}

fn top_url(browser: &Browser) -> Option<String> {
    let frame = browser.main_frame()?;
    Some(CefString::from(&frame.url()).to_string())
}

fn ask(slot: &std::rc::Rc<Slot>, browser: &Browser, requesting: Option<&CefString>, frame_url: Option<String>) -> Ask {
    let origin = requesting
        .map(ToString::to_string)
        .zip(top_url(browser))
        .and_then(|(requesting, top)| eligible_origin(&requesting, frame_url.as_deref(), &top));
    Ask {
        browser: std::rc::Rc::downgrade(slot),
        queue: queue_id(browser.identifier()),
        partition: slot.partition.borrow().as_ref().map(partition),
        origin,
        kinds: None,
        prompt: None,
    }
}

wrap_permission_handler! {
    pub(crate) struct Handler { slot: std::rc::Rc<Slot> }
    impl PermissionHandler {
        fn on_request_media_access_permission(
            &self, browser: Option<&mut Browser>, frame: Option<&mut Frame>,
            requesting_origin: Option<&CefString>, requested_permissions: u32,
            callback: Option<&mut MediaAccessCallback>,
        ) -> ::std::os::raw::c_int {
            let (Some(browser), Some(callback)) = (browser, callback) else { return 0 };
            let frame_url = frame.map(|frame| CefString::from(&frame.url()).to_string());
            let mut ask = ask(&self.slot, browser, requesting_origin, frame_url);
            ask.kinds = media_kinds(requested_permissions);
            let answer = Box::new(MediaAnswer { callback: callback.clone(), requested: requested_permissions });
            request(&services::of(&self.slot), ask, answer);
            1
        }

        fn on_show_permission_prompt(
            &self, browser: Option<&mut Browser>, prompt_id: u64,
            requesting_origin: Option<&CefString>, requested_permissions: u32,
            callback: Option<&mut PermissionPromptCallback>,
        ) -> ::std::os::raw::c_int {
            let (Some(browser), Some(callback)) = (browser, callback) else { return 0 };
            let mut ask = ask(&self.slot, browser, requesting_origin, None);
            ask.kinds = prompt_kinds(requested_permissions);
            ask.prompt = Some(prompt_id);
            request(&services::of(&self.slot), ask, Box::new(PromptAnswer { callback: callback.clone() }));
            1
        }

        fn on_dismiss_permission_prompt(
            &self, _browser: Option<&mut Browser>, prompt_id: u64, _result: PermissionRequestResult,
        ) {
            dismissed(&services::of(&self.slot), prompt_id);
        }
    }
}

pub(crate) fn handler(slot: std::rc::Rc<Slot>) -> PermissionHandler {
    Handler::new(slot)
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use flecha_permissions::{Decision, SiteControl};

    use super::*;

    type Log = Rc<RefCell<Vec<(&'static str, Option<Verdict>)>>>;

    struct Recorder {
        name: &'static str,
        log: Log,
        answered: bool,
    }

    impl Answer for Recorder {
        fn answer(mut self: Box<Self>, verdict: Verdict) {
            self.answered = true;
            self.log.borrow_mut().push((self.name, Some(verdict)));
        }
    }

    impl Drop for Recorder {
        fn drop(&mut self) {
            if !self.answered {
                self.log.borrow_mut().push((self.name, None));
            }
        }
    }

    fn recorder(name: &'static str, log: &Log) -> Box<Recorder> {
        Box::new(Recorder { name, log: log.clone(), answered: false })
    }

    fn camera_ask(queue: &str, origin: &str) -> Ask {
        Ask {
            browser: Weak::new(),
            queue: QueueId::new(queue),
            partition: Some(Partition::Shared),
            origin: eligible_origin(origin, Some(origin), origin),
            kinds: media_kinds(MEDIA_VIDEO),
            prompt: None,
        }
    }

    fn events(services: &Services) -> Vec<Option<RequestId>> {
        std::iter::from_fn(|| services.permission_events.pop())
            .map(|(_, prompt)| prompt.map(|prompt| prompt.request))
            .collect()
    }

    #[test]
    fn origins_are_checked_before_anything_is_granted() {
        let secure = "https://meet.example/room";
        assert!(eligible_origin("https://meet.example/", Some(secure), secure).is_some());
        assert!(eligible_origin("http://localhost:8080/", None, "http://localhost:8080/").is_some());
        // Opaque requesters, such as sandboxed frames, never qualify.
        assert!(eligible_origin("null", Some(secure), secure).is_none());
        assert!(eligible_origin("data:text/html,hi", None, secure).is_none());
        assert!(eligible_origin("file:///tmp/x.html", None, secure).is_none());
        // Insecure requester, or a secure frame inside an insecure page.
        assert!(eligible_origin("http://meet.example/", None, "http://meet.example/").is_none());
        assert!(eligible_origin("https://meet.example/", None, "http://news.example/").is_none());
        // The frame's document must be the requesting origin.
        assert!(eligible_origin("https://meet.example/", Some("https://evil.example/"), secure).is_none());
        assert!(eligible_origin("https://meet.example/", Some("about:blank"), secure).is_none());
    }

    #[test]
    fn unknown_kinds_deny_the_whole_request() {
        let desktop =
            sys::cef_media_access_permission_types_t::CEF_MEDIA_PERMISSION_DESKTOP_VIDEO_CAPTURE as u32;
        assert_eq!(media_kinds(MEDIA_AUDIO | MEDIA_VIDEO).map(|set| set.iter().count()), Some(2));
        assert_eq!(media_kinds(desktop), None);
        assert_eq!(media_kinds(MEDIA_VIDEO | desktop), None);
        assert_eq!(media_kinds(0), None);
        let midi = sys::cef_permission_request_types_t::CEF_PERMISSION_TYPE_MIDI_SYSEX as u32;
        let geo = sys::cef_permission_request_types_t::CEF_PERMISSION_TYPE_GEOLOCATION as u32;
        assert!(prompt_kinds(geo).is_some_and(|set| set.contains(PermissionKind::Geolocation)));
        assert_eq!(prompt_kinds(geo | midi), None);
    }

    #[test]
    fn ineligible_requests_are_denied_at_once() {
        let services = Services::new();
        let log = Log::default();
        let mut sandboxed = camera_ask("browser-1", "https://meet.example/");
        sandboxed.origin = eligible_origin("null", Some("https://meet.example/"), "https://meet.example/");
        request(&services, sandboxed, recorder("sandboxed", &log));
        let mut no_partition = camera_ask("browser-1", "https://meet.example/");
        no_partition.partition = None;
        request(&services, no_partition, recorder("unplaced", &log));
        assert_eq!(*log.borrow(), [("sandboxed", Some(Verdict::Block)), ("unplaced", Some(Verdict::Block))]);
        assert!(events(&services).is_empty());
    }

    #[test]
    fn prompts_queue_per_browser_and_answer_every_waiter_once() {
        let services = Services::new();
        let log = Log::default();
        request(&services, camera_ask("browser-1", "https://meet.example/"), recorder("first", &log));
        request(&services, camera_ask("browser-1", "https://meet.example/"), recorder("joined", &log));
        let front = services.permissions.borrow().pending(&QueueId::new("browser-1")).unwrap();
        assert!(log.borrow().is_empty(), "held until the banner is answered");
        assert_eq!(events(&services), [Some(front.request)]);

        resolve(&services, front.request, PromptOutcome::AllowOnce).unwrap();
        assert_eq!(*log.borrow(), [("first", Some(Verdict::Allow)), ("joined", Some(Verdict::Allow))]);
        assert_eq!(events(&services), [None]);
        assert_eq!(
            resolve(&services, front.request, PromptOutcome::AllowOnce),
            Err(PermissionError::UnknownRequest)
        );

        // The one-time answer holds for the page.
        request(&services, camera_ask("browser-1", "https://meet.example/"), recorder("again", &log));
        assert_eq!(log.borrow().last(), Some(&("again", Some(Verdict::Allow))));
    }

    #[test]
    fn leaving_the_page_or_closing_the_tab_denies_what_it_holds() {
        let services = Services::new();
        let log = Log::default();
        request(&services, camera_ask("browser-1", "https://meet.example/"), recorder("navigated", &log));
        request(&services, camera_ask("browser-2", "https://meet.example/"), recorder("other tab", &log));
        leave(&services, &QueueId::new("browser-1"));
        assert_eq!(*log.borrow(), [("navigated", Some(Verdict::Block))]);
        assert!(services.permissions.borrow().pending(&QueueId::new("browser-1")).is_none());
        assert!(services.permissions.borrow().pending(&QueueId::new("browser-2")).is_some());
    }

    #[test]
    fn a_prompt_cef_dismissed_is_dropped_unanswered_and_later_dismissals_are_no_ops() {
        let services = Services::new();
        let log = Log::default();
        let mut ask = camera_ask("browser-1", "https://meet.example/");
        ask.prompt = Some(41);
        request(&services, ask, recorder("dismissed", &log));
        dismissed(&services, 41);
        assert_eq!(*log.borrow(), [("dismissed", None)]);
        assert!(services.permissions.borrow().pending(&QueueId::new("browser-1")).is_none());
        dismissed(&services, 41);

        // A prompt the engine answered first: CEF's later dismissal changes nothing.
        let mut ask = camera_ask("browser-1", "https://meet.example/");
        ask.prompt = Some(42);
        request(&services, ask, recorder("answered", &log));
        let front = services.permissions.borrow().pending(&QueueId::new("browser-1")).unwrap();
        resolve(&services, front.request, PromptOutcome::BlockOnce).unwrap();
        dismissed(&services, 42);
        assert_eq!(log.borrow().last(), Some(&("answered", Some(Verdict::Block))));
        assert_eq!(log.borrow().len(), 2);
    }

    #[test]
    fn a_site_panel_change_answers_the_requests_it_decides() {
        let services = Services::new();
        let log = Log::default();
        request(&services, camera_ask("browser-1", "https://meet.example/"), recorder("held", &log));
        let origin = Origin::parse("https://meet.example/").unwrap();
        apply(&services, |manager| {
            let completions = manager
                .set_setting(&Partition::Shared, &origin, SiteControl::Media, Decision::Block)
                .unwrap();
            ((), completions)
        });
        assert_eq!(*log.borrow(), [("held", Some(Verdict::Block))]);
    }
}
