//! Engine state that every browser's callbacks share with the engine, beyond popups.
//!
//! Everything here lives on CEF's UI thread except [`Services::adblock`], which the IO-thread
//! resource filter reads. A callback that calls back into CEF must release its `RefCell` borrow
//! first: CEF can re-enter a handler synchronously, and a second borrow aborts the process.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::{Rc, Weak};

use crate::callbacks::{Slot, Stage};

use flecha_adblock::AdBlocker;
use flecha_boosts::Boosts;
use flecha_core::TabId;

use crate::context_menu::ContextMenuEvent;
use crate::devtools::{DevToolsEvent, Internal};
use crate::downloads::{DownloadEvent, Downloads};
use crate::find::FindEvent;
use crate::permissions::{PermissionPrompt, Permissions};

pub(crate) struct Services {
    pub adblock: AdBlocker,
    pub boosts: RefCell<Boosts>,
    pub downloads: RefCell<Downloads>,
    pub download_events: Mailbox<DownloadEvent>,
    pub permissions: RefCell<Permissions>,
    pub permission_events: Mailbox<Option<PermissionPrompt>>,
    pub context_menu_events: RefCell<VecDeque<ContextMenuEvent>>,
    pub find_events: RefCell<VecDeque<FindEvent>>,
    pub devtools_events: Mailbox<DevToolsEvent>,
    /// The next CDP message id. CDP ids are positive 32-bit integers.
    pub devtools_next: Cell<i32>,
    /// CDP calls the engine made itself, by message id with the browser they went to. The shell
    /// never sees their replies.
    pub devtools_internal: RefCell<BTreeMap<i32, (Weak<Slot>, Internal)>>,
}

impl Services {
    pub fn new() -> Self {
        // Blocking is on, as Settings' default, and blocks nothing until rules are loaded.
        let (adblock, _) = AdBlocker::new(true, &[]);
        Self {
            adblock,
            boosts: RefCell::new(Boosts::new()),
            downloads: RefCell::new(Downloads::default()),
            download_events: Mailbox::default(),
            permissions: RefCell::new(Permissions::default()),
            permission_events: Mailbox::default(),
            context_menu_events: RefCell::default(),
            find_events: RefCell::default(),
            devtools_events: Mailbox::default(),
            devtools_next: Cell::new(1),
            devtools_internal: RefCell::new(BTreeMap::new()),
        }
    }

    pub fn next_devtools_id(&self) -> i32 {
        let id = self.devtools_next.get();
        self.devtools_next
            .set(id.checked_add(1).filter(|next| *next > 0).unwrap_or(1));
        id
    }
}

/// Events addressed to the browser that raised them. The tab is read when the shell polls, so
/// an event follows its browser through [`crate::Engine::transfer`].
pub(crate) struct Mailbox<E> {
    queue: RefCell<VecDeque<(Weak<Slot>, E)>>,
}

impl<E> Default for Mailbox<E> {
    fn default() -> Self {
        Self {
            queue: RefCell::new(VecDeque::new()),
        }
    }
}

impl<E> Mailbox<E> {
    pub fn push(&self, slot: Weak<Slot>, event: E) {
        self.queue.borrow_mut().push_back((slot, event));
    }

    /// The next event, with the tab of its browser while the engine still owns that browser.
    pub fn pop(&self) -> Option<(Option<TabId>, E)> {
        let (slot, event) = self.queue.borrow_mut().pop_front()?;
        Some((live_tab(&slot), event))
    }
}

/// The tab a browser belongs to, unless the engine closed it or CEF destroyed it.
pub(crate) fn live_tab(slot: &Weak<Slot>) -> Option<TabId> {
    let slot = slot.upgrade()?;
    if slot.retired.get() || slot.stage.get() == Stage::Closed {
        return None;
    }
    Some(slot.key.borrow().tab.clone())
}

/// The services `slot`'s callbacks share with the engine.
pub(crate) fn of(slot: &Slot) -> Rc<Services> {
    slot.popups.services.clone()
}
