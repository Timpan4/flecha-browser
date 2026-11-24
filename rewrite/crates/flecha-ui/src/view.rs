use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;

use flecha_cef::{BrowserKey, Owner, Viewport};
use flecha_core::{FolderId, TabId, WindowId};
use flecha_surface::{Frame, input, surface};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, ClickEvent, Context, Div, Entity, FocusHandle, InteractiveElement as _,
    IntoElement, KeyUpEvent, MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent,
    MouseUpEvent, ParentElement as _, Pixels, Render, RenderImage, ScrollEvent,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, canvas, deferred, div, px,
    relative, rgb,
};

use crate::{Action, Shared, dispatch};
use crate::{
    command_bar::CommandBar, downloads_view::DownloadsPanel, history_view::HistoryPanel,
    settings_view::SettingsPanel, shortcuts::Bindings, text_field::TextField,
};

/// A popup's read-only address bar, as tall as the Peek toolbar.
pub(crate) const POPUP_BAR_HEIGHT: Pixels = px(40.);

#[derive(Default)]
pub(crate) enum Overlay {
    #[default]
    None,
    Command(CommandBar),
    History(HistoryPanel),
    Settings(Box<SettingsPanel>),
    Archive(TextField),
    Folder(Option<FolderId>, TextField),
    TabActions(TabId),
    Downloads(DownloadsPanel),
    SiteControls,
}

impl Overlay {
    pub(crate) fn is_open(&self) -> bool {
        !matches!(self, Self::None)
    }
}

pub(crate) struct Shell {
    pub(crate) state: Shared,
    pub(crate) id: WindowId,
    focus: FocusHandle,
    page_focus: FocusHandle,
    selected_pane: Option<(Owner, TabId)>,
    focused_browser: Option<BrowserKey>,
    pub(crate) overlay: Overlay,
    pub(crate) bindings: Bindings,
    pub(crate) consumed_keys: BTreeSet<String>,
    images: BTreeMap<TabId, Vec<Arc<RenderImage>>>,
    title: String,
    _activation: Subscription,
}

impl Shell {
    pub fn new(state: Shared, id: WindowId, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let focus = cx.focus_handle();
        let page_focus = cx.focus_handle();
        let bindings = Bindings::new(&state.borrow().model.core.settings.keybindings);
        focus.focus(window, cx);
        cx.new(|cx| {
            state
                .borrow_mut()
                .sleep
                .window_activation(&id, window.is_window_active());
            let activation = cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                let active = window.is_window_active();
                this.state
                    .borrow_mut()
                    .sleep
                    .window_activation(&this.id, active);
                if active {
                    this.state.borrow_mut().model.core.focus_window(&this.id);
                } else {
                    this.consumed_keys.clear();
                }
                let _ = this.sync_page_focus(window, cx);
            });
            Self {
                state,
                id,
                focus,
                page_focus,
                selected_pane: None,
                focused_browser: None,
                overlay: Overlay::None,
                bindings,
                consumed_keys: BTreeSet::new(),
                images: BTreeMap::new(),
                title: String::new(),
                _activation: activation,
            }
        })
    }

    pub(crate) fn action_button(
        &self,
        id: &'static str,
        label: &'static str,
        action: Action,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        button(id, label).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            dispatch(&this.state, &this.id, action.clone(), cx);
        }))
    }

    pub(crate) fn show_overlay(
        &mut self,
        overlay: Overlay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.overlay = overlay;
        self.focus.focus(window, cx);
        let _ = self.sync_page_focus(window, cx);
        cx.notify();
    }

    pub(crate) fn dismiss_overlay(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Overlay::Settings(panel) = &mut self.overlay
            && !panel.prepare_close()
        {
            cx.notify();
            return;
        }
        if self.overlay.is_open() {
            self.overlay = Overlay::None;
            if self.selected_pane.is_some() {
                self.page_focus.focus(window, cx);
            }
            let _ = self.sync_page_focus(window, cx);
            cx.notify();
        }
    }

    pub(crate) fn navigate_from_overlay(
        &mut self,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_overlay(window, cx);
        if !matches!(action, Action::Peek(_))
            && self.state.borrow().model.peeks.contains_key(&self.id)
        {
            dispatch(&self.state, &self.id, Action::ClosePeek, cx);
        }
        dispatch(&self.state, &self.id, action, cx);
    }

    pub(crate) fn sync_page_focus(
        &mut self,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<BrowserKey> {
        let state = self.state.borrow();
        let pane = state.model.keyboard_pane(&self.id);
        let selection = pane
            .as_ref()
            .map(|(owner, tab)| (owner.clone(), (*tab).clone()));
        if self.selected_pane != selection {
            self.selected_pane = selection;
            if self.selected_pane.is_some() && !self.overlay.is_open() {
                self.page_focus.focus(window, cx);
            } else {
                self.focus.focus(window, cx);
            }
        }

        let focused = pane.and_then(|(owner, tab)| {
            if window.is_window_active()
                && self.page_focus.is_focused(window)
                && state.engine.owner(tab) == Some(&owner)
                && state.model.page(&owner, tab).is_some()
            {
                state.engine.key(tab)
            } else {
                None
            }
        });
        if self.focused_browser != focused {
            // A transfer can leave the previous browser live in another window.
            if let Some(old) = self.focused_browser.take()
                && state.engine.key(&old.tab).as_ref() == Some(&old)
                && matches!(
                    state.engine.owner(&old.tab),
                    Some(Owner::Tab(id) | Owner::Peek(id) | Owner::Flechinha(id) | Owner::Popup(id)) if id == &self.id
                )
            {
                state.engine.focused(&old.tab, false);
            }
            if let Some(browser) = &focused {
                state.engine.focused(&browser.tab, true);
            }
            self.focused_browser = focused.clone();
        }
        focused
    }

    pub(crate) fn page_key(
        &self,
        focused: Option<BrowserKey>,
        stroke: &gpui::Keystroke,
        up: bool,
        repeat: bool,
        character_input: bool,
    ) {
        let host = focused.and_then(|key| self.state.borrow().engine.host(&key.tab));
        if let Some(host) = host {
            let altgr = character_input && stroke.modifiers.control && stroke.modifiers.alt;
            input::key(&host, stroke, up, repeat, altgr);
        }
    }

    fn pane(
        &self,
        owner: Owner,
        id: TabId,
        images: &mut BTreeMap<TabId, Vec<Arc<RenderImage>>>,
        cx: &Context<Self>,
    ) -> Div {
        let (frames, prompt, writable) = {
            let state = self.state.borrow();
            (
                state.engine.frames(&id).unwrap_or_default(),
                state.engine.pending_permission_prompt(&id),
                state.permissions.error().is_none(),
            )
        };
        let pane_images = [
            frames.view.as_ref(),
            frames.popup.as_ref().map(|(_, frame)| frame),
        ]
        .into_iter()
        .flatten()
        .filter_map(Frame::image)
        .cloned()
        .collect();
        images.insert(id.clone(), pane_images);
        let state = self.state.clone();
        let origin = Rc::new(Cell::new(gpui::Point::default()));
        let mut element = div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x171e22))
            .child(surface(frames.view, frames.popup))
            .child(
                canvas(|_, _, _| (), {
                    let origin = origin.clone();
                    let owner = owner.clone();
                    let id = id.clone();
                    move |pane, (), window, cx| {
                        origin.set(pane.origin);
                        #[cfg(target_os = "linux")]
                        state.borrow().engine.follow_window_device(
                            flecha_surface::SurfaceTarget::for_window(window),
                        );
                        let screen = window
                            .display(cx)
                            .map_or(window.bounds(), |display| display.bounds());
                        let viewport =
                            Viewport::new(pane, window.bounds(), screen, window.scale_factor());
                        let (state, owner, id) = (state.clone(), owner.clone(), id.clone());
                        cx.defer(move |cx| {
                            let mut state = state.borrow_mut();
                            if let Err(error) = state.pane(owner, id, viewport) {
                                state.fail(error, cx);
                            }
                        });
                    }
                })
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            );
        macro_rules! pointer {
            ($event:ty, $handler:ident) => {{
                let (id, origin) = (id.clone(), origin.clone());
                cx.listener(move |this, event: &$event, _, _| {
                    if let Some(host) = this.state.borrow().engine.host(&id) {
                        input::$handler(&host, event, origin.get());
                    }
                })
            }};
        }
        element = element
            .on_mouse_move(pointer!(MouseMoveEvent, mouse_move))
            .on_mouse_exit(pointer!(MouseExitEvent, mouse_exit))
            .on_scroll(pointer!(ScrollEvent, scroll));
        for button in [MouseButton::Left, MouseButton::Middle, MouseButton::Right] {
            let (down_id, down_origin, down_owner) = (id.clone(), origin.clone(), owner.clone());
            let (up_id, up_origin) = (id.clone(), origin.clone());
            element = element
                .on_mouse_down(
                    button,
                    cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        if matches!(down_owner, Owner::Tab(_))
                            && this.state.borrow().model.active(&this.id) != Some(&down_id)
                        {
                            dispatch(&this.state, &this.id, Action::Select(down_id.clone()), cx);
                        }
                        this.page_focus.focus(window, cx);
                        let _ = this.sync_page_focus(window, cx);
                        let state = this.state.borrow();
                        if let Some(host) = state.engine.host(&down_id) {
                            input::click(
                                &host,
                                event.position - down_origin.get(),
                                event.modifiers,
                                event.button,
                                event.click_count,
                                false,
                            );
                        }
                    }),
                )
                .on_mouse_up(
                    button,
                    cx.listener(move |this, event: &MouseUpEvent, _, _| {
                        if let Some(host) = this.state.borrow().engine.host(&up_id) {
                            input::click(
                                &host,
                                event.position - up_origin.get(),
                                event.modifiers,
                                event.button,
                                event.click_count,
                                true,
                            );
                        }
                    }),
                );
        }
        div()
            .flex()
            .flex_col()
            .size_full()
            .when_some(prompt, |pane, prompt| {
                pane.child(self.permission_banner(prompt, writable, cx))
            })
            .child(div().flex_1().min_h_0().child(element))
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.bindings
            .refresh(&self.state.borrow().model.core.settings.keybindings);
        let _ = self.sync_page_focus(window, cx);
        let popup_title = (self.state.borrow().model.popups.get(&self.id))
            .map(|popup| popup.page.tab.title.clone());
        if let Some(title) = popup_title
            && title != self.title
        {
            window.set_window_title(&title);
            self.title = title;
        }
        let mut main = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x161c20))
            .text_color(rgb(0xe2edf1))
            .on_key_down(cx.listener(|this, event, window, cx| this.key(event, window, cx)))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, window, cx| {
                if this.consumed_keys.remove(&event.keystroke.key) || this.overlay.is_open() {
                    cx.stop_propagation();
                    return;
                }
                let focused = this.sync_page_focus(window, cx);
                this.page_key(focused, &event.keystroke, true, false, false);
            }))
            .child(div().absolute().size_0().track_focus(&self.page_focus))
            .child(div().absolute().size_0().track_focus(&self.focus));
        let mut images = BTreeMap::new();
        let state = self.state.borrow();
        if let Some(popup) = state.model.popups.get(&self.id) {
            main = main
                .child(
                    div()
                        .h(POPUP_BAR_HEIGHT)
                        .flex_shrink_0()
                        .px_2()
                        .flex()
                        .items_center()
                        .min_w_0()
                        .child(div().truncate().child(popup.page.tab.url.clone())),
                )
                .child(div().flex_1().min_h_0().child(self.pane(
                    Owner::Popup(self.id.clone()),
                    popup.page.id.clone(),
                    &mut images,
                    cx,
                )));
        } else if let Some(page) = state.model.flechinhas.get(&self.id) {
            main = main
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .p_2()
                        .child(self.action_button("return", "Open in Flecha", Action::Return, cx))
                        .child(self.action_button("close", "Close", Action::Close, cx)),
                )
                .child(div().flex_1().min_h_0().child(self.pane(
                    Owner::Flechinha(self.id.clone()),
                    page.id.clone(),
                    &mut images,
                    cx,
                )));
        } else if let Some(current) = state.model.core.window(&self.id) {
            let core = &state.model.core;
            let active_space = &current.active_space_id;
            let view = current.visible_tabs();
            let mut pages = div().flex().size_full().overflow_hidden();
            for (index, tab) in view.iter().enumerate() {
                let pane = self.pane(Owner::Tab(self.id.clone()), tab.clone(), &mut images, cx);
                let ratio = current
                    .spaces
                    .get(active_space)
                    .and_then(|space| space.split_ratio)
                    .unwrap_or(0.5) as f32;
                let width = if view.len() == 1 {
                    1.0
                } else if index == 0 {
                    ratio
                } else {
                    1.0 - ratio
                };
                pages = pages.child(div().h_full().w(relative(width)).child(pane));
            }
            let mut page_area = div().relative().flex_1().min_h_0().min_w_0().child(pages);
            if let Some(peek) = state.model.peeks.get(&self.id) {
                page_area = page_area.child(deferred(
                    div()
                        .absolute()
                        .left(relative(0.08))
                        .right(relative(0.08))
                        .top(px(8.))
                        .bottom(px(24.))
                        .rounded(px(12.))
                        .overflow_hidden()
                        .occlude()
                        .flex()
                        .flex_col()
                        .bg(rgb(0x26333a))
                        .child(
                            div()
                                .h(px(40.))
                                .flex_shrink_0()
                                .px_2()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(self.action_button(
                                    "promote",
                                    "Open in tab",
                                    Action::Promote,
                                    cx,
                                ))
                                .child(self.action_button(
                                    "close-peek",
                                    "Close Peek",
                                    Action::ClosePeek,
                                    cx,
                                )),
                        )
                        .child(div().flex_1().min_h_0().child(self.pane(
                            Owner::Peek(self.id.clone()),
                            peek.id.clone(),
                            &mut images,
                            cx,
                        ))),
                ));
            }
            let sidebar = self.sidebar(&state, current, cx);
            let mut body = div().flex_1().min_h_0().relative().flex();
            if core.sidebar_visible(&self.id) {
                body = body.child(sidebar).child(page_area);
            } else {
                body = body.child(page_area).child(sidebar);
            }
            main = main.child(body);
        }
        if !state.status.is_empty() {
            main = main.child(div().p_2().child(state.status.clone()));
        }
        if state.model.peeks.contains_key(&self.id) {
            main = main.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .bg(gpui::rgba(0x00000088))
                    .on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        dispatch(&this.state, &this.id, Action::ClosePeek, cx);
                        cx.stop_propagation();
                    })),
            );
        }
        drop(state);
        if let Overlay::History(panel) = &mut self.overlay {
            panel.refresh(&self.state.borrow().history);
        }
        if let Overlay::Settings(panel) = &mut self.overlay {
            panel.refresh(&self.state.borrow());
        }
        if let Overlay::Downloads(panel) = &mut self.overlay {
            panel.refresh(&self.state.borrow().downloads);
        }
        let overlay = match &self.overlay {
            Overlay::None => None,
            Overlay::Command(bar) => Some(self.command_panel(bar, cx)),
            Overlay::History(panel) => Some(self.history_panel(panel, cx)),
            Overlay::Settings(panel) => Some(self.settings_panel(panel, cx)),
            Overlay::Archive(input) => Some(self.archive_panel(input, cx)),
            Overlay::Folder(id, input) => Some(self.folder_panel(id.as_ref(), input, cx)),
            Overlay::TabActions(id) => Some(self.tab_actions_panel(id, cx)),
            Overlay::Downloads(panel) => Some(self.downloads_panel(panel, cx)),
            Overlay::SiteControls => Some(self.site_controls_panel(cx)),
        };
        if let Some(panel) = overlay {
            let drawer = matches!(self.overlay, Overlay::Archive(_) | Overlay::Downloads(_));
            let site_controls = matches!(self.overlay, Overlay::SiteControls);
            main = main.child(deferred(
                div()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .when(!site_controls, |backdrop| {
                        backdrop.bg(gpui::rgba(0x00000088))
                    })
                    .child(div().absolute().inset_0().on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.dismiss_overlay(window, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(
                        div()
                            .relative()
                            .flex()
                            .justify_center()
                            .pt_10()
                            .px_4()
                            .h_full()
                            .pb_4()
                            .when(drawer, |frame| frame.justify_end().pt_4())
                            .when(site_controls, |frame| frame.justify_start())
                            .child(panel.when(drawer, |drawer| drawer.h_full().w(px(420.)))),
                    ),
            ));
        }
        for (tab, retired) in std::mem::replace(&mut self.images, images) {
            for image in retired {
                if !self
                    .images
                    .get(&tab)
                    .is_some_and(|next| next.iter().any(|next| next.id == image.id))
                {
                    let _ = window.drop_image(image);
                }
            }
        }
        main
    }
}

pub(crate) fn overlay_panel() -> Div {
    div()
        .flex()
        .flex_col()
        .min_h_0()
        .max_h_full()
        .self_start()
        .w(px(560.))
        .max_w_full()
        .occlude()
        .gap_2()
        .p_3()
        .text_sm()
        .rounded_xl()
        .border_1()
        .border_color(rgb(0x405863))
        .bg(rgb(0x1d2a32))
        .shadow_lg()
}

pub(crate) fn button(
    id: impl Into<gpui::ElementId>,
    name: impl Into<gpui::SharedString>,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .hover(|style| style.bg(rgb(0x37545e)))
        .child(name.into())
}
