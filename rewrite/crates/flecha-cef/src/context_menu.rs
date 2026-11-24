use std::cell::{Cell, RefCell};
use std::rc::Rc;

use cef::{rc::Rc as _, *};
use gpui::{Pixels, Point as GpuiPoint};

use crate::callbacks::Slot;
use crate::{BrowserKey, Engine};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextMenuId {
    pub key: BrowserKey,
    serial: u64,
}

#[derive(Debug)]
pub struct ContextMenuRequest {
    pub id: ContextMenuId,
    pub position: (i32, i32),
    pub anchor: GpuiPoint<Pixels>,
    pub context: MenuContext,
}

#[derive(Debug, Default)]
pub struct MenuContext {
    /// The address of the page the menu opened on, its top frame's.
    pub page_url: String,
    pub link_url: String,
    /// The element's source: an image's, or a video's or audio's.
    pub src_url: String,
    /// Whether the menu opened on an image, which `src_url` names.
    pub is_image: bool,
    pub selection_text: String,
    pub is_editable: bool,
    /// Chromium's verdict on copying the selection. It refuses a password field's.
    pub can_copy: bool,
    /// Chromium's verdict on cutting the selection. It refuses a password field's.
    pub can_cut: bool,
    pub can_go_back: bool,
    pub can_go_forward: bool,
}

pub enum ContextMenuEvent {
    Open(ContextMenuRequest),
    Dismissed(ContextMenuId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameEdit {
    Copy,
    Cut,
    Paste,
    SelectAll,
}

/// The frame a context menu opened on, used to re-check focus when a queued menu edit runs.
#[derive(Clone)]
pub struct MenuFrame(Frame);

impl MenuFrame {
    pub fn is_focused(&self) -> bool {
        self.0.is_valid() != 0 && self.0.is_focused() != 0
    }
}

struct Pending {
    id: ContextMenuId,
    frame: Frame,
}

#[derive(Default)]
pub(crate) struct Menu {
    serial: Cell<u64>,
    pending: RefCell<Option<Pending>>,
}

wrap_context_menu_handler! {
    struct Handler { slot: Rc<Slot> }
    impl ContextMenuHandler {
        fn run_context_menu(
            &self, browser: Option<&mut Browser>, frame: Option<&mut Frame>,
            params: Option<&mut ContextMenuParams>, _model: Option<&mut MenuModel>,
            callback: Option<&mut RunContextMenuCallback>,
        ) -> i32 {
            let Some(callback) = callback else { return 1 };
            if let (Some(browser), Some(frame), Some(params)) = (browser, frame, params)
                && !self.slot.retired.get()
            {
                cancel(&self.slot);
                let serial = self.slot.menu.serial.get().wrapping_add(1);
                self.slot.menu.serial.set(serial);
                let id = ContextMenuId { key: self.slot.key.borrow().clone(), serial };
                let position = (params.xcoord(), params.ycoord());
                let edit = params.edit_state_flags().as_ref().0;
                let can = |flag: sys::cef_context_menu_edit_state_flags_t| edit & flag.0 != 0;
                let request = ContextMenuRequest {
                    id: id.clone(),
                    position,
                    anchor: self.slot.viewport.get().window_point(position),
                    context: MenuContext {
                        page_url: CefString::from(&params.page_url()).to_string(),
                        link_url: CefString::from(&params.link_url()).to_string(),
                        src_url: CefString::from(&params.source_url()).to_string(),
                        is_image: *params.media_type().as_ref()
                            == sys::cef_context_menu_media_type_t::CM_MEDIATYPE_IMAGE,
                        selection_text: CefString::from(&params.selection_text()).to_string(),
                        is_editable: params.is_editable() != 0,
                        can_copy: can(sys::cef_context_menu_edit_state_flags_t::CM_EDITFLAG_CAN_COPY),
                        can_cut: can(sys::cef_context_menu_edit_state_flags_t::CM_EDITFLAG_CAN_CUT),
                        can_go_back: browser.can_go_back() != 0,
                        can_go_forward: browser.can_go_forward() != 0,
                    },
                };
                self.slot.menu.pending.replace(Some(Pending {
                    id,
                    frame: frame.clone(),
                }));
                emit(&self.slot, ContextMenuEvent::Open(request));
            }
            // CEF can cancel its menu on focus loss. Finish its callback here and let GPUI
            // own the captured menu until the user dismisses it or the source page goes away.
            callback.cancel();
            1
        }
    }
}

pub(crate) fn handler(slot: Rc<Slot>) -> ContextMenuHandler {
    Handler::new(slot)
}

fn emit(slot: &Slot, event: ContextMenuEvent) {
    slot.popups
        .services
        .context_menu_events
        .borrow_mut()
        .push_back(event);
}

pub(crate) fn cancel(slot: &Slot) {
    if let Some(pending) = slot.menu.pending.take() {
        emit(slot, ContextMenuEvent::Dismissed(pending.id));
    }
}

impl Engine {
    pub fn next_context_menu_event(&mut self) -> Option<ContextMenuEvent> {
        self.services().context_menu_events.borrow_mut().pop_front()
    }

    fn menu_slot(&self, id: &ContextMenuId) -> Option<&Slot> {
        let slot = &self.browsers.get(&id.key.tab)?.slot;
        let current = slot.menu.pending.borrow();
        current.as_ref().filter(|pending| &pending.id == id)?;
        Some(slot)
    }

    pub fn context_menu_is_open(&self, id: &ContextMenuId) -> bool {
        self.menu_slot(id).is_some()
    }

    pub fn dismiss_context_menu(&self, id: &ContextMenuId) {
        if let Some(slot) = self.menu_slot(id) {
            cancel(slot);
        }
    }

    /// Runs Chromium's own edit command in the menu's frame and closes the menu.
    pub fn context_menu_edit(&self, id: &ContextMenuId, action: FrameEdit) {
        self.in_menu_frame(id, |frame| match action {
            FrameEdit::Copy => frame.copy(),
            FrameEdit::Cut => frame.cut(),
            FrameEdit::Paste => frame.paste(),
            FrameEdit::SelectAll => frame.select_all(),
        });
    }

    /// Whether the menu's frame still has focus. Typed input, such as a shell paste, and the
    /// shell's selection read go to the focused frame, so a page that moves focus while the menu
    /// is open would otherwise redirect them.
    pub fn context_menu_frame_focused(&self, id: &ContextMenuId) -> bool {
        self.context_menu_frame(id)
            .is_some_and(|frame| frame.is_focused())
    }

    /// The menu's frame, when the menu is still open and its frame is valid.
    pub fn context_menu_frame(&self, id: &ContextMenuId) -> Option<MenuFrame> {
        self.menu_frame(id).map(MenuFrame)
    }

    fn menu_frame(&self, id: &ContextMenuId) -> Option<Frame> {
        let slot = self.menu_slot(id)?;
        let frame = slot.menu.pending.borrow().as_ref()?.frame.clone();
        (frame.is_valid() != 0).then_some(frame)
    }

    fn in_menu_frame(&self, id: &ContextMenuId, edit: impl FnOnce(&Frame)) {
        if let Some(frame) = self.menu_frame(id) {
            edit(&frame);
        }
        self.dismiss_context_menu(id);
    }
}
