mod callbacks;
mod identity;
mod popup;
mod pump;
mod runtime;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::{Rc, Weak};

use anyhow::{Context as _, Result, ensure};
use cef::{
    BrowserHost, BrowserSettings, CefString, ImplBrowser as _, ImplBrowserHost as _,
    ImplFrame as _, Rect, RequestContext, RequestContextSettings, RuntimeStyle, WindowInfo,
    browser_host_create_browser_sync, request_context_create_context,
};
use flecha_core::TabId;
use gpui::{Bounds, Pixels};

pub use callbacks::Frames;
use callbacks::{Queue, Slot, Stage};
use identity::Generations;
pub use identity::{BrowserKey, Owner, Partition};
pub use popup::{Disposition, Opened};
use popup::{Pending, Popups};
pub use runtime::Runtime;
use runtime::path_string;

/// `Opened`: CEF is creating this browser for a page, and the engine already owns it.
/// `ClosedByPage`: the page closed its browser (`window.close()`), or CEF abandoned
/// creating it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Created(i32),
    Closed,
    Address(String),
    Title(String),
    Loaded { url: String, status: i32 },
    LoadFailed { url: String, code: i32 },
    Paint,
    PaintFailed,
    Opened(Opened),
    ClosedByPage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrowserEvent {
    pub key: BrowserKey,
    pub event: Event,
}

#[derive(Clone, Copy, PartialEq)]
pub struct Viewport {
    origin: (i32, i32),
    size: (i32, i32),
    screen: (i32, i32, i32, i32),
    scale: f32,
}

impl Viewport {
    pub fn new(
        pane: Bounds<Pixels>,
        window: Bounds<Pixels>,
        screen: Bounds<Pixels>,
        scale: f32,
    ) -> Self {
        let pixels = |px| f32::from(px).round() as i32;
        Self {
            origin: (
                pixels(window.origin.x + pane.origin.x),
                pixels(window.origin.y + pane.origin.y),
            ),
            size: (
                pixels(pane.size.width).max(1),
                pixels(pane.size.height).max(1),
            ),
            screen: (
                pixels(screen.origin.x),
                pixels(screen.origin.y),
                pixels(screen.size.width),
                pixels(screen.size.height),
            ),
            scale,
        }
    }

    fn view_rect(self) -> Rect {
        Rect {
            width: self.size.0,
            height: self.size.1,
            ..Rect::default()
        }
    }

    fn screen_rect(self) -> Rect {
        let (x, y, width, height) = self.screen;
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn scale(self) -> f32 {
        self.scale
    }

    fn screen_origin(self) -> (i32, i32) {
        self.origin
    }
}

struct Session {
    owner: Owner,
    partition: Partition,
    slot: Rc<Slot>,
    hidden: bool,
    opener: Option<Weak<Slot>>,
}

struct PersistentContext {
    handle: RequestContext,
    ready: Rc<Cell<bool>>,
}

pub struct Engine {
    profile: PathBuf,
    contexts: BTreeMap<Partition, PersistentContext>,
    contexts_changed: Rc<Cell<bool>>,
    generations: Generations,
    browsers: BTreeMap<TabId, Session>,
    closing: BTreeSet<BrowserKey>,
    events: Queue,
    popups: Rc<Popups>,
}

impl Engine {
    /// `mint` names the tabs of browsers that pages open.
    pub fn new(profile: PathBuf, mint: fn() -> TabId) -> Self {
        let generations = Generations::default();
        let popups = Rc::new(Popups {
            counter: generations.counter(),
            mint,
            pending: RefCell::new(Vec::new()),
            live: Cell::new(0),
        });
        Self {
            profile,
            contexts: BTreeMap::new(),
            contexts_changed: Rc::new(Cell::new(false)),
            generations,
            browsers: BTreeMap::new(),
            closing: BTreeSet::new(),
            events: Queue::default(),
            popups,
        }
    }

    pub fn key(&self, tab: &TabId) -> Option<BrowserKey> {
        self.generations.current(tab)
    }

    pub fn owner(&self, tab: &TabId) -> Option<&Owner> {
        self.browsers.get(tab).map(|session| &session.owner)
    }

    pub fn partition(&self, tab: &TabId) -> Option<&Partition> {
        self.browsers.get(tab).map(|session| &session.partition)
    }

    pub fn ensure(
        &mut self,
        tab: TabId,
        owner: Owner,
        partition: Partition,
        url: &str,
        viewport: Viewport,
    ) -> Result<()> {
        if let Some(session) = self.browsers.get_mut(&tab)
            && session.partition == partition
        {
            session.owner = owner;
            self.resize(&tab, viewport);
            return Ok(());
        }
        self.close(&tab);
        let Some(mut context) = self.context(&partition)? else {
            return Ok(());
        };
        let key = self.generations.create(tab.clone());
        let slot = Rc::new(Slot::new(
            key.clone(),
            viewport,
            self.events.clone(),
            self.popups.clone(),
        ));
        let mut client = callbacks::client(slot.clone());
        let info = WindowInfo {
            runtime_style: RuntimeStyle::ALLOY,
            windowless_rendering_enabled: 1,
            ..Default::default()
        };
        let browser = browser_host_create_browser_sync(
            Some(&info),
            Some(&mut client),
            Some(&CefString::from(url)),
            Some(&BrowserSettings::default()),
            None,
            Some(&mut context),
        );
        if browser.is_none() {
            self.generations.retire(&key);
            anyhow::bail!("CEF could not create an OSR browser");
        }
        self.browsers.insert(
            tab.clone(),
            Session {
                slot,
                owner,
                partition,
                hidden: false,
                opener: None,
            },
        );
        let alloy = browser
            .and_then(|browser| browser.host())
            .is_some_and(|host| host.runtime_style() == RuntimeStyle::ALLOY);
        ensure!(alloy, "CEF did not create an Alloy OSR browser");
        Ok(())
    }

    fn context(&mut self, partition: &Partition) -> Result<Option<RequestContext>> {
        if *partition == Partition::Shared {
            return cef::request_context_get_global_context()
                .map(Some)
                .context("get shared CEF request context");
        }
        if !self.contexts.contains_key(partition) {
            let settings = RequestContextSettings {
                cache_path: path_string(&self.profile.join(partition.directory()))?,
                persist_session_cookies: 1,
                ..Default::default()
            };
            let ready = Rc::new(Cell::new(false));
            let mut handler =
                callbacks::ContextReady::new(ready.clone(), self.contexts_changed.clone());
            let handle = request_context_create_context(Some(&settings), Some(&mut handler))
                .context("create isolated CEF request context")?;
            self.contexts
                .insert(partition.clone(), PersistentContext { handle, ready });
        }
        let context = &self.contexts[partition];
        Ok(context.ready.get().then(|| context.handle.clone()))
    }

    pub fn contexts_changed(&self) -> bool {
        self.contexts_changed.replace(false)
    }

    pub fn resize(&self, tab: &TabId, viewport: Viewport) {
        let Some(session) = self.browsers.get(tab) else {
            return;
        };
        let old = session.slot.viewport.replace(viewport);
        if old != viewport
            && let Some(host) = self.host(tab)
        {
            if (old.scale, old.screen) != (viewport.scale, viewport.screen) {
                host.notify_screen_info_changed();
            }
            if (old.size, old.scale) != (viewport.size, viewport.scale) {
                host.was_resized();
            }
        }
    }

    pub fn frames(&self, tab: &TabId) -> Option<Frames> {
        Some(self.browsers.get(tab)?.slot.surface.borrow().clone())
    }

    pub fn host(&self, tab: &TabId) -> Option<BrowserHost> {
        self.browsers
            .get(tab)?
            .slot
            .browser
            .borrow()
            .as_ref()?
            .host()
    }

    pub fn browser_id(&self, tab: &TabId) -> Option<i32> {
        Some(
            self.browsers
                .get(tab)?
                .slot
                .browser
                .borrow()
                .as_ref()?
                .identifier(),
        )
    }

    pub fn viewport(&self, tab: &TabId) -> Option<Viewport> {
        Some(self.browsers.get(tab)?.slot.viewport.get())
    }

    /// Exercise the same queue and dispatch boundary as CEF's callbacks.
    #[doc(hidden)]
    pub fn queue_verification_callbacks(&self, tab: &TabId) -> Option<BrowserKey> {
        let slot = &self.browsers.get(tab)?.slot;
        slot.emit(Event::Paint);
        slot.emit(Event::Title("retired verification title".into()));
        slot.emit(Event::Address("about:blank#retired-verification".into()));
        Some(slot.key.borrow().clone())
    }

    pub fn ids(&self) -> impl Iterator<Item = &TabId> {
        self.browsers.keys()
    }

    pub fn focused(&self, tab: &TabId, focused: bool) {
        if let Some(host) = self.host(tab) {
            host.set_focus(i32::from(focused));
        }
    }

    pub fn set_visible(&mut self, tab: &TabId, visible: bool) {
        let Some(session) = self.browsers.get_mut(tab) else {
            return;
        };
        if session.hidden == visible {
            session.hidden = !visible;
            if let Some(host) = self.host(tab) {
                host.was_hidden(i32::from(!visible));
            }
        }
    }

    pub fn release_contexts(&mut self) {
        assert!(self.is_drained());
        self.contexts.clear();
    }

    pub fn transfer(&mut self, from: &TabId, to: TabId, owner: Owner) {
        let Some(mut session) = self.browsers.remove(from) else {
            return;
        };
        if from != &to {
            let mut key = session.slot.key.borrow_mut();
            *key = self.generations.transfer(&key, to.clone());
            session.slot.paint_pending.set(false);
        }
        session.owner = owner;
        self.browsers.insert(to, session);
    }

    pub fn execute_script(&self, tab: &TabId, script: &str) -> bool {
        let frame = self
            .browsers
            .get(tab)
            .and_then(|session| session.slot.browser.borrow().as_ref()?.main_frame());
        if let Some(frame) = frame {
            frame.execute_java_script(Some(&CefString::from(script)), None, 0);
            true
        } else {
            false
        }
    }

    pub fn close(&mut self, tab: &TabId) {
        let Some(session) = self.browsers.remove(tab) else {
            return;
        };
        self.shut(&session.slot);
    }

    fn shut(&mut self, slot: &Slot) {
        let key = slot.key.borrow().clone();
        self.generations.retire(&key);
        slot.retired.set(true);
        match slot.stage.get() {
            // CEF already destroyed it. Its queued ClosedByPage now has a retired key.
            Stage::Closed => {}
            // on_after_created closes it, or CEF reports the aborted creation.
            Stage::Pending => {
                self.closing.insert(key);
            }
            Stage::Live => {
                self.closing.insert(key);
                let host = slot
                    .browser
                    .borrow()
                    .as_ref()
                    .and_then(|browser| browser.host());
                if let Some(host) = host {
                    host.close_browser(1);
                }
            }
        }
    }

    /// Registers the browsers CEF started creating for pages since the last call. A
    /// popup whose opener has gone since is closed.
    fn adopt_popups(&mut self) {
        let pending = std::mem::take(&mut *self.popups.pending.borrow_mut());
        for Pending { child, opener } in pending {
            let source = opener.key.borrow().tab.clone();
            let session = self
                .browsers
                .get(&source)
                .filter(|session| Rc::ptr_eq(&session.slot, &opener));
            let Some((owner, partition)) =
                session.map(|session| (session.owner.clone(), session.partition.clone()))
            else {
                self.shut(&child);
                continue;
            };
            let key = child.key.borrow().clone();
            self.generations.register(&key);
            self.browsers.insert(
                key.tab,
                Session {
                    owner,
                    // CEF creates a popup in its opener's request context.
                    partition,
                    slot: child,
                    hidden: false,
                    opener: Some(Rc::downgrade(&opener)),
                },
            );
        }
    }

    /// The tab whose page opened this one, while that page is still open.
    pub fn opener(&self, tab: &TabId) -> Option<TabId> {
        let opener = self.browsers.get(tab)?.opener.as_ref()?.upgrade()?;
        let source = opener.key.borrow().tab.clone();
        self.browsers
            .get(&source)
            .filter(|session| Rc::ptr_eq(&session.slot, &opener))
            .map(|_| source)
    }

    /// CEF browsers between `on_after_created` and `on_before_close`, counted by the
    /// callbacks themselves.
    pub fn live_browsers(&self) -> usize {
        self.popups.live.get()
    }

    pub fn close_all(&mut self) {
        self.adopt_popups();
        while let Some(tab) = self.browsers.first_key_value().map(|(tab, _)| tab.clone()) {
            self.close(&tab);
        }
    }

    /// CEF must call `on_before_close` for every browser before the app exits.
    pub fn is_drained(&self) -> bool {
        self.browsers.is_empty()
            && self.closing.is_empty()
            && self.popups.pending.borrow().is_empty()
            && self.live_browsers() == 0
    }

    pub fn next_event(&mut self) -> Option<BrowserEvent> {
        self.adopt_popups();
        loop {
            let message = self.events.borrow_mut().pop_front()?;
            if message.event == Event::Closed && self.closing.remove(&message.key) {
                // Always finish a retired generation's shutdown bookkeeping.
                if self.generations.current(&message.key.tab).is_none() {
                    return Some(message);
                }
                continue;
            }
            if !self.generations.accepts(&message.key) {
                continue;
            }
            if message.event == Event::Paint
                && let Some(session) = self.browsers.get(&message.key.tab)
            {
                session.slot.paint_pending.set(false);
            }
            if message.event == Event::ClosedByPage {
                self.browsers.remove(&message.key.tab);
                self.generations.retire(&message.key);
            }
            return Some(message);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! callback_samples {
        ($($pattern:pat => $sample:expr),+ $(,)?) => {{
            fn cover(event: &Event) {
                match event { $($pattern => (),)+ }
            }
            let samples = [$($sample,)+];
            samples.iter().for_each(cover);
            samples
        }};
    }

    #[test]
    fn rejects_every_callback_from_a_retired_generation() {
        let mut engine = Engine::new(PathBuf::new(), || TabId::new("popup"));
        let tab = TabId::new("replaced-tab");
        let retired = engine.generations.create(tab.clone());
        let current = engine.generations.create(tab.clone());
        engine.closing.insert(retired.clone());

        let url = "https://example.test/old";
        for event in callback_samples![
            Event::Created(_) => Event::Created(1),
            Event::Closed => Event::Closed,
            Event::Address(_) => Event::Address(url.into()),
            Event::Title(_) => Event::Title("Old document".into()),
            Event::Loaded { .. } => Event::Loaded { url: url.into(), status: 200 },
            Event::LoadFailed { .. } => Event::LoadFailed { url: url.into(), code: -106 },
            Event::Paint => Event::Paint,
            Event::PaintFailed => Event::PaintFailed,
            Event::Opened(_) => Event::Opened(Opened {
                url: url.into(),
                disposition: Disposition::Popup,
                size: Some((480, 360)),
            }),
            Event::ClosedByPage => Event::ClosedByPage,
        ] {
            engine.events.borrow_mut().push_back(BrowserEvent {
                key: retired.clone(),
                event: event.clone(),
            });
            assert_eq!(
                engine.next_event(),
                None,
                "retired callback reached the shell: {event:?}"
            );
        }

        assert!(engine.closing.is_empty());
        let live = BrowserEvent {
            key: current,
            event: Event::Paint,
        };
        engine.events.borrow_mut().push_back(live.clone());
        assert_eq!(engine.next_event(), Some(live));
    }
}
