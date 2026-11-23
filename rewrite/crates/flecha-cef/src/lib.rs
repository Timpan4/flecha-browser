mod callbacks;
mod identity;
mod pump;
mod runtime;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context as _, Result, ensure};
use cef::{
    BrowserHost, BrowserSettings, CefString, ImplBrowser as _, ImplBrowserHost as _, Rect,
    RequestContext, RequestContextSettings, RuntimeStyle, WindowInfo,
    browser_host_create_browser_sync, request_context_create_context,
};
use flecha_core::TabId;
use gpui::{Bounds, Pixels};

pub use callbacks::Frames;
use callbacks::{Queue, Slot};
use identity::Generations;
pub use identity::{BrowserKey, Owner, Partition};
pub use runtime::Runtime;
use runtime::path_string;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Created(i32),
    Closed,
    Address(String),
    Title(String),
    Loaded { url: String, status: i32 },
    Paint,
    PaintFailed,
    OpenLink { url: String, foreground: bool },
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
}

pub struct Engine {
    profile: PathBuf,
    contexts: BTreeMap<Partition, RequestContext>,
    generations: Generations,
    browsers: BTreeMap<TabId, Session>,
    closing: BTreeSet<BrowserKey>,
    events: Queue,
}

impl Engine {
    pub fn new(profile: PathBuf) -> Self {
        Self {
            profile,
            contexts: BTreeMap::new(),
            generations: Generations::default(),
            browsers: BTreeMap::new(),
            closing: BTreeSet::new(),
            events: Queue::default(),
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
        let mut context = self.context(&partition)?;
        let key = self.generations.create(tab.clone());
        let slot = Rc::new(Slot {
            key: key.clone(),
            browser: RefCell::new(None),
            viewport: Cell::new(viewport),
            surface: RefCell::new(Frames::default()),
            retired: Cell::new(false),
            paint_pending: Cell::new(false),
            events: self.events.clone(),
        });
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
            },
        );
        let alloy = browser
            .and_then(|browser| browser.host())
            .is_some_and(|host| host.runtime_style() == RuntimeStyle::ALLOY);
        ensure!(alloy, "CEF did not create an Alloy OSR browser");
        Ok(())
    }

    fn context(&mut self, partition: &Partition) -> Result<RequestContext> {
        if *partition == Partition::Shared {
            return cef::request_context_get_global_context()
                .context("get shared CEF request context");
        }
        if let Some(context) = self.contexts.get(partition) {
            return Ok(context.clone());
        }
        let settings = RequestContextSettings {
            cache_path: path_string(&self.profile.join(partition.directory()))?,
            persist_session_cookies: 1,
            ..Default::default()
        };
        let context = request_context_create_context(Some(&settings), None)
            .context("create isolated CEF request context")?;
        self.contexts.insert(partition.clone(), context.clone());
        Ok(context)
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

    pub fn close(&mut self, tab: &TabId) {
        let Some(session) = self.browsers.remove(tab) else {
            return;
        };
        let slot = &session.slot;
        self.generations.retire(&slot.key);
        slot.retired.set(true);
        let host = slot
            .browser
            .borrow()
            .as_ref()
            .and_then(|browser| browser.host());
        if let Some(host) = host {
            self.closing.insert(slot.key.clone());
            host.close_browser(1);
        }
    }

    pub fn close_all(&mut self) {
        while let Some(tab) = self.browsers.first_key_value().map(|(tab, _)| tab.clone()) {
            self.close(&tab);
        }
    }

    pub fn is_drained(&self) -> bool {
        self.browsers.is_empty() && self.closing.is_empty()
    }

    pub fn take_events(&mut self) -> Vec<BrowserEvent> {
        let events = self.events.take();
        events
            .into_iter()
            .filter(|message| {
                match message.event {
                    Event::Closed if self.closing.remove(&message.key) => return true,
                    Event::Paint => {
                        if let Some(session) = self.browsers.get(&message.key.tab)
                            && session.slot.key == message.key
                        {
                            session.slot.paint_pending.set(false);
                        }
                    }
                    _ => {}
                }
                self.generations.accepts(&message.key)
            })
            .collect()
    }
}
