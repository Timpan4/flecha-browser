use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use flecha_cef::{BrowserKey, ContextMenuRequest, Owner, Viewport};
use flecha_core::{FolderId, TabId, WindowId};
use flecha_surface::{Frame, input, surface};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, App, ClickEvent, Context, Div, FocusHandle, FontWeight,
    HoverListenerMode, InteractiveElement as _, IntoElement, KeyUpEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, ParentElement as _, Pixels,
    Render, RenderImage, ScrollEvent, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, canvas, deferred, div, px, relative,
};

use crate::icons::icon;
use crate::shortcuts::Navigation;
use crate::theme::{DEFAULT_TINT, Palette, palette};
use crate::tokens::{self, ease};
use crate::{Action, Shared, dispatch};
use crate::{
    command_bar::{CommandBar, SuggestionClient},
    downloads_view::DownloadsPanel,
    find_bar::FindSession,
    history_view::HistoryPanel,
    settings_view::SettingsPanel,
    shortcuts::Bindings,
    text_field::TextField,
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
    ContextMenu(ContextMenuRequest),
}

impl Overlay {
    pub(crate) fn is_open(&self) -> bool {
        !matches!(self, Self::None)
    }
}

pub(crate) struct Shell {
    pub(crate) state: Shared,
    pub(crate) id: WindowId,
    pub(crate) focus: FocusHandle,
    pub(crate) page_focus: FocusHandle,
    selected_pane: Option<(Owner, TabId)>,
    focused_browser: Option<BrowserKey>,
    pub(crate) overlay: Overlay,
    pub(crate) find: Option<FindSession>,
    pub(crate) suggestion_client: Arc<SuggestionClient>,
    pub(crate) last_suggestion_sent: Option<Instant>,
    pub(crate) bindings: Bindings,
    pub(crate) consumed_keys: BTreeSet<String>,
    pub(crate) hovered_tab_row: Option<TabId>,
    pub(crate) tab_drag: Option<crate::tab_drag::TabDrag>,
    images: BTreeMap<TabId, Vec<Arc<RenderImage>>>,
    title: String,
    native_fullscreen: crate::fullscreen::NativeFullscreen,
    _activation: Subscription,
    _appearance: Subscription,
}

/// The first window's content until CEF has started and a main window takes it over.
pub(crate) struct Starting;

impl Render for Starting {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full()
    }
}

impl Shell {
    pub fn build(state: Shared, id: WindowId, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let page_focus = cx.focus_handle();
        let bindings = Bindings::new(&state.borrow().model.core.settings.keybindings);
        focus.focus(window, cx);
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
                this.state.borrow_mut().model.focus_window(&this.id);
            } else {
                this.consumed_keys.clear();
                this.cancel_tab_drag(window, cx);
            }
            let _ = this.sync_page_focus(window, cx);
        });
        // Theme::System follows the desktop's light or dark appearance as it changes.
        let appearance = cx.observe_window_appearance(window, |_, _, cx| cx.notify());
        Self {
            state,
            id,
            focus,
            page_focus,
            selected_pane: None,
            focused_browser: None,
            overlay: Overlay::None,
            find: None,
            suggestion_client: Arc::default(),
            last_suggestion_sent: None,
            bindings,
            consumed_keys: BTreeSet::new(),
            hovered_tab_row: None,
            tab_drag: None,
            images: BTreeMap::new(),
            title: String::new(),
            native_fullscreen: Default::default(),
            _activation: activation,
            _appearance: appearance,
        }
    }

    /// A `.text-button` that dispatches `action`.
    pub(crate) fn action_button(
        &self,
        id: &'static str,
        label: &'static str,
        action: Action,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        text_button(id, label).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            dispatch(&this.state, &this.id, action.clone(), cx);
        }))
    }

    pub(crate) fn show_overlay(
        &mut self,
        overlay: Overlay,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_context_menu();
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
            self.cancel_context_menu();
            self.overlay = Overlay::None;
            if self.selected_pane.is_some() {
                self.page_focus.focus(window, cx);
            }
            let _ = self.sync_page_focus(window, cx);
            cx.notify();
        }
    }

    fn cancel_context_menu(&self) {
        if let Overlay::ContextMenu(request) = &self.overlay {
            self.state.borrow().engine.dismiss_context_menu(&request.id);
        }
    }

    pub(crate) fn active_browser(&self) -> Option<BrowserKey> {
        let state = self.state.borrow();
        let (owner, tab) = state.model.keyboard_pane(&self.id)?;
        if state.engine.owner(tab) != Some(&owner) || state.model.page(&owner, tab).is_none() {
            return None;
        }
        state.engine.key(tab)
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
        let selection = self
            .state
            .borrow()
            .model
            .keyboard_pane(&self.id)
            .map(|(owner, tab)| (owner, tab.clone()));
        let browser = self.active_browser();
        let find_left = self
            .find
            .as_ref()
            .is_some_and(|find| browser.as_ref() != Some(&find.browser));
        let menu_left = matches!(&self.overlay, Overlay::ContextMenu(request) if browser.as_ref() != Some(&request.id.key));
        if find_left {
            self.stop_find();
        }
        if menu_left {
            self.cancel_context_menu();
            self.overlay = Overlay::None;
        }
        if self.selected_pane != selection || find_left || menu_left {
            self.selected_pane = selection;
            if self.selected_pane.is_some() && !self.overlay.is_open() {
                self.page_focus.focus(window, cx);
            } else {
                self.focus.focus(window, cx);
            }
        }

        let focused =
            browser.filter(|_| window.is_window_active() && self.page_focus.is_focused(window));
        if self.focused_browser != focused {
            let state = self.state.borrow();
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

    /// Records where this window shows pages, so an agent's background page takes that size
    /// while the window shows none.
    fn page_area_probe(&self) -> impl IntoElement {
        let (state, id) = (self.state.clone(), self.id.clone());
        canvas(
            |_, _, _| (),
            move |area, (), window, cx| {
                let screen = window
                    .display(cx)
                    .map_or(window.bounds(), |display| display.bounds());
                let viewport = Viewport::new(area, window.bounds(), screen, window.scale_factor());
                let (state, id) = (state.clone(), id.clone());
                cx.defer(move |_| state.borrow_mut().page_area(id, viewport));
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full()
    }

    /// Puts the window in native fullscreen while its page is in HTML fullscreen, and takes it
    /// out afterwards unless the user had made it fullscreen first. GPUI redraws a window whose
    /// bounds change, as a native fullscreen change does, so a late entry is undone here too.
    fn sync_native_fullscreen(&mut self, page_fullscreen: bool, window: &mut Window) {
        let (next, toggle) = self
            .native_fullscreen
            .next(page_fullscreen, window.is_fullscreen());
        self.native_fullscreen = next;
        if toggle {
            window.toggle_fullscreen();
        }
    }

    pub(crate) fn page_key(
        &self,
        focused: Option<BrowserKey>,
        stroke: &gpui::Keystroke,
        up: bool,
        repeat: bool,
        character_input: bool,
    ) {
        let Some(key) = focused else {
            return;
        };
        let altgr = character_input && stroke.modifiers.control && stroke.modifiers.alt;
        let mut state = self.state.borrow_mut();
        // Behind a pending copy or cut, the key waits so it cannot change what the read finds.
        let held = crate::clipboard::Held::Key {
            stroke: stroke.clone(),
            up,
            repeat,
            altgr,
        };
        if state.hold(&key, held) {
            return;
        }
        if let Some(host) = state.engine.host(&key.tab) {
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
        let (frames, prompt, writable, (cursor, status)) = {
            let state = self.state.borrow();
            (
                state.engine.frames(&id).unwrap_or_default(),
                state.engine.pending_permission_prompt(&id),
                state.permissions.error().is_none(),
                state.engine.pointer_state(&id),
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
        let group = SharedString::new_static("page-pointer");
        let mut element = div()
            .id(format!("page-viewport-{}", id.as_str()))
            .group(group.clone())
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(palette().page)
            .cursor(cursor)
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
                cx.listener(move |this, event: &$event, _, cx| {
                    if this.tab_drag.is_some() && cx.has_active_drag() {
                        return;
                    }
                    if let Some(host) = this.state.borrow().engine.host(&id) {
                        input::$handler(&host, event, origin.get());
                    }
                })
            }};
        }
        element = element
            .on_mouse_move(pointer!(MouseMoveEvent, mouse_move))
            .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
            .on_hover({
                let (id, origin) = (id.clone(), origin.clone());
                cx.listener(move |this, hovered: &bool, window, _| {
                    if !hovered {
                        let state = this.state.borrow();
                        state.engine.clear_pointer_status(&id);
                        if let Some(host) = state.engine.host(&id) {
                            input::mouse_leave(
                                &host,
                                window.mouse_position() - origin.get(),
                                window.modifiers(),
                            );
                        }
                    }
                })
            })
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
                    cx.listener(move |this, event: &MouseUpEvent, _, cx| {
                        if this.tab_drag.is_some() && cx.has_active_drag() {
                            return;
                        }
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
        if let Some(find) = &self.find
            && find.browser.tab == id
        {
            element = element.child(self.find_panel(find, cx));
        }
        if let Some(status) = status {
            element = element.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .max_w_full()
                    .invisible()
                    .group_hover(group, |style| style.visible())
                    .rounded_tr(px(tokens::RADIUS_SM))
                    .px(px(tokens::SPACE_2))
                    .py(px(tokens::SPACE_1))
                    .bg(palette().surface)
                    .text_color(palette().text_2)
                    .text_size(px(tokens::TEXT_XS))
                    .truncate()
                    .child(status),
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
        if !cx.has_active_drag() {
            self.tab_drag = None;
        }
        let palette_now = {
            let state = self.state.borrow();
            let incognito = state.model.is_incognito(&self.id);
            // A popup or Flechinha window wears the tint of the Space the main window shows,
            // as Flechinha's page does (chromium/shell/flechinhaApi.ts:36).
            let core = state.model.state(&self.id).unwrap_or(&state.model.core);
            let current = core
                .window(&self.id)
                .unwrap_or_else(|| core.focused_window());
            let space = core.space(&current.active_space_id);
            Palette::resolve(
                state.model.core.settings.theme,
                window.appearance(),
                incognito,
                space.map_or(DEFAULT_TINT, |space| space.color.as_str()),
            )
        };
        crate::theme::draw_with(palette_now);
        self.bindings
            .refresh(&self.state.borrow().model.core.settings.keybindings);
        self.sync_command_preferences(cx);
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
            // The window wears the Space's tint around the sidebar and the page, and the body
            // text is `font: 400 var(--text-md) / 1.35` (ui/styles.css:17).
            .bg(palette().tint)
            .text_color(palette().text)
            .when_some(ui_font(cx), |main, font| main.font_family(font))
            .text_size(px(tokens::TEXT_MD))
            .line_height(relative(tokens::LINE_HEIGHT))
            .on_any_mouse_down(cx.listener(|this, event: &MouseDownEvent, _, cx| {
                let navigation = match event.button {
                    MouseButton::Navigate(NavigationDirection::Back) => Navigation::Back,
                    MouseButton::Navigate(NavigationDirection::Forward) => Navigation::Forward,
                    _ => return,
                };
                cx.stop_propagation();
                dispatch(&this.state, &this.id, Action::Browser(navigation), cx);
            }))
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
        let fullscreen = self.state.borrow().fullscreen_pane(&self.id);
        self.sync_native_fullscreen(fullscreen.is_some(), window);
        let state = self.state.borrow();
        if let Some((owner, tab)) = fullscreen.clone() {
            let mut body = div().flex_1().min_h_0().relative().flex();
            if let Some(core) = state.model.state(&self.id)
                && let Some(current) = core.window(&self.id)
                && crate::fullscreen::keeps_sidebar(
                    &state.model.core.settings,
                    core.sidebar_visible(&self.id),
                )
            {
                body = body.child(self.sidebar(&state, current, cx));
            } else {
                self.hovered_tab_row = None;
            }
            let page = self.pane(owner, tab, &mut images, cx);
            main = main.child(body.child(div().flex_1().min_w_0().min_h_0().child(page)));
        } else if let Some(popup) = state.model.popups.get(&self.id) {
            main = main
                .child(
                    div()
                        .h(POPUP_BAR_HEIGHT)
                        .flex_shrink_0()
                        .px(px(tokens::SPACE_2))
                        .flex()
                        .items_center()
                        .gap(px(tokens::SPACE_2))
                        .min_w_0()
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_color(palette().text_2)
                                .child(popup.page.tab.url.clone()),
                        )
                        .children(self.zoom_badge(cx)),
                )
                .child(div().flex_1().min_h_0().child(self.pane(
                    Owner::Popup(self.id.clone()),
                    popup.page.id.clone(),
                    &mut images,
                    cx,
                )));
        } else if let Some(page) = state.model.flechinhas.get(&self.id) {
            let host = flecha_core::suggestions::display_url(&page.tab.url);
            main = main
                .child(
                    // Flechinha's toolbar (ui/flechinha.css): 44 tall on the tint, controls 28
                    // tall, the address a resting chip.
                    div()
                        .h(px(tokens::FLECHINHA_TOOLBAR))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(tokens::TOOLBAR_GAP))
                        .pl(px(tokens::SPACE_2 - tokens::TOOLBAR_GAP))
                        .pr(px(tokens::SPACE_2))
                        .child(window_close("close").on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                dispatch(&this.state, &this.id, Action::Close, cx);
                            },
                        )))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .h(px(tokens::PILL_HEIGHT))
                                .mx(px(tokens::FLECHINHA_ADDRESS_MARGIN))
                                .px(px(tokens::URLBAR_PAD_LEFT))
                                .flex()
                                .items_center()
                                .gap(px(tokens::SPACE_2))
                                .rounded(px(tokens::PAGE_RADIUS))
                                .bg(palette().fill_subtle)
                                .shadow(vec![hairline(palette().line)])
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(host),
                                )
                                .when(!page.tab.title.is_empty(), |address| {
                                    address.child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .truncate()
                                            .text_color(palette().text_3)
                                            .child(format!("— {}", page.tab.title)),
                                    )
                                }),
                        )
                        .children(self.zoom_badge(cx))
                        .child(
                            div()
                                .id("return")
                                .flex()
                                .items_center()
                                .gap(px(tokens::PILL_GAP))
                                .h(px(tokens::PILL_HEIGHT))
                                .flex_shrink_0()
                                .pl(px(tokens::FLECHINHA_CTA_PAD_LEFT))
                                .pr(px(tokens::FLECHINHA_CTA_PAD_RIGHT))
                                .rounded(px(tokens::PAGE_RADIUS))
                                .bg(palette().cta)
                                .text_color(palette().cta_text)
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_size(px(tokens::TEXT_SM))
                                .cursor_pointer()
                                .hover(|cta| cta.bg(palette().cta_hover))
                                .child(icon("external", tokens::PILL_ICON, palette().cta_text))
                                .child("Open in Flecha")
                                .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                    dispatch(&this.state, &this.id, Action::Return, cx);
                                })),
                        ),
                )
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px(px(tokens::PAGE_GAP))
                        .pb(px(tokens::PAGE_GAP))
                        .child(page_card(self.pane(
                            Owner::Flechinha(self.id.clone()),
                            page.id.clone(),
                            &mut images,
                            cx,
                        ))),
                );
        } else if let Some(core) = state.model.state(&self.id)
            && let Some(current) = core.window(&self.id)
        {
            let active_space = &current.active_space_id;
            let view = current.visible_tabs();
            // The page area, kFlechaGap in from the window's edges and flush with a docked
            // sidebar (FlechaFrameState::PageInsets, patch 0010). Split panes keep
            // kFlechaSplitGap between them (patch 0019), so each takes its share of the rest.
            let mut pages = div()
                .relative()
                .flex()
                .size_full()
                .gap(px(tokens::SPLIT_GAP))
                .child(self.page_area_probe());
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
                pages = pages.child(
                    div()
                        .h_full()
                        .min_w_0()
                        .flex_basis(relative(width))
                        .child(page_card(pane)),
                );
            }
            if !self.overlay.is_open()
                && !state.model.peeks.contains_key(&self.id)
                && let Some(target) = self
                    .dragged_tab(core)
                    .and_then(|source| core.page_split_target(&self.id, source))
            {
                pages = pages.child(self.page_split_drop(target, cx));
            }
            if let Some(peek) = state.model.peeks.get(&self.id) {
                // Peek (patch 0026): the page inset 8% of the area's width on each side, 48
                // from its top and 24 from its bottom, with 12px corners; its toolbar floats
                // 6px above it (ui/app.ts:1524-1532 and ui/styles.css:1168-1178).
                let card = div()
                    .absolute()
                    .left(relative(0.08))
                    .right(relative(0.08))
                    .top(px(48.))
                    .bottom(px(24.))
                    .occlude()
                    .child(page_card(self.pane(
                        Owner::Peek(self.id.clone()),
                        peek.id.clone(),
                        &mut images,
                        cx,
                    )));
                let bar =
                    div()
                        .id("peek-bar")
                        .absolute()
                        .left(relative(0.08))
                        .right(relative(0.08))
                        .top(px(48. - tokens::PEEK_BAR - 6.))
                        .h(px(tokens::PEEK_BAR))
                        .occlude()
                        .flex()
                        .items_center()
                        .gap(px(tokens::PILL_GAP))
                        .pl(px(tokens::PEEK_BAR_PAD_LEFT))
                        .pr(px(tokens::PEEK_BAR_PAD_RIGHT))
                        .rounded(px(tokens::RADIUS_MD))
                        .bg(palette().surface)
                        .shadow(palette().shadow_pop())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(palette().text_2)
                                .font_weight(FontWeight::MEDIUM)
                                .child(crate::sidebar::tab_label(&peek.tab)),
                        )
                        .children(self.zoom_badge(cx))
                        .child(
                            pill_button("promote", "Open as Tab", Some("external")).on_click(
                                cx.listener(|this, _: &ClickEvent, _, cx| {
                                    dispatch(&this.state, &this.id, Action::Promote, cx);
                                }),
                            ),
                        )
                        .child(icon_button("close-peek", "close", "Close Peek").on_click(
                            cx.listener(|this, _: &ClickEvent, _, cx| {
                                dispatch(&this.state, &this.id, Action::ClosePeek, cx);
                            }),
                        ));
                pages = pages.child(deferred(
                    div()
                        .absolute()
                        .inset_0()
                        // The page paints over the bar's shadow, as the shipping Peek's native
                        // page sits above the sidebar page that draws the bar.
                        .child(bar)
                        .child(card)
                        .with_animation(
                            "peek-open",
                            Animation::new(tokens::MEDIUM).with_easing(ease),
                            |peek, delta| peek.opacity(delta),
                        ),
                ));
            }
            let docked = core.sidebar_visible(&self.id);
            if !docked {
                self.hovered_tab_row = None;
            }
            let page_area = div()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .pt(px(tokens::PAGE_GAP))
                .pr(px(tokens::PAGE_GAP))
                .pb(px(tokens::PAGE_GAP))
                .when(!docked, |area| area.pl(px(tokens::PAGE_GAP)))
                .child(pages);
            let sidebar = self.sidebar(&state, current, cx);
            let mut body = div().flex_1().min_h_0().relative().flex();
            if docked {
                body = body.child(sidebar).child(page_area);
            } else {
                body = body.child(page_area).child(sidebar);
            }
            main = main.child(body);
        }
        if let Some(status) = state.status_for(&self.id) {
            main = main.child(
                div()
                    .px(px(tokens::SPACE_3))
                    .pb(px(tokens::SPACE_2))
                    .text_size(px(tokens::TEXT_SM))
                    .text_color(palette().text_2)
                    .child(status.to_owned()),
            );
        }
        if fullscreen.is_none() && state.model.peeks.contains_key(&self.id) {
            main = main.child(
                div()
                    .id("peek-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .occlude()
                    .bg(palette().peek_backdrop)
                    .on_any_mouse_down(cx.listener(|this, _: &MouseDownEvent, _, cx| {
                        dispatch(&this.state, &this.id, Action::ClosePeek, cx);
                        cx.stop_propagation();
                    }))
                    .with_animation(
                        "peek-backdrop-fade",
                        Animation::new(tokens::MEDIUM).with_easing(ease),
                        |backdrop, delta| backdrop.opacity(delta),
                    ),
            );
        }
        let sidebar_width = state.model.core.settings.sidebar_width.pixels() as f32;
        let docked = state
            .model
            .state(&self.id)
            .is_some_and(|core| core.sidebar_visible(&self.id));
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
            Overlay::None | Overlay::ContextMenu(_) => None,
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
            let placement = match self.overlay {
                Overlay::Command(_) => Placement::Palette,
                Overlay::Settings(_) | Overlay::History(_) => Placement::Page,
                Overlay::Archive(_) | Overlay::Downloads(_) => Placement::SidebarFoot,
                Overlay::SiteControls => Placement::SidebarTop(tokens::SITE_PANEL_TOP),
                _ => Placement::SidebarTop(tokens::SPACE_EDITOR_TOP),
            };
            let page_left = if docked {
                sidebar_width
            } else {
                tokens::PAGE_GAP
            };
            let frame = match placement {
                // #palette and .palette-box (ui/styles.css:1212-1249).
                Placement::Palette => div()
                    .absolute()
                    .top(relative(tokens::PALETTE_TOP / 100.))
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .child(
                        panel
                            .w(px(tokens::PALETTE_WIDTH))
                            .max_w(relative(0.9))
                            .with_animation(
                                "palette-drop",
                                Animation::new(tokens::MEDIUM).with_easing(ease),
                                |panel, delta| panel.opacity(delta).mt(px(-6. * (1. - delta))),
                            ),
                    )
                    .into_any_element(),
                // Settings and History are pages of their own in the shipping shell, shown in
                // the page area.
                Placement::Page => div()
                    .absolute()
                    .top(px(tokens::PAGE_GAP))
                    .bottom(px(tokens::PAGE_GAP))
                    .right(px(tokens::PAGE_GAP))
                    .left(px(page_left))
                    .flex()
                    .child(
                        panel
                            .flex_1()
                            .h_full()
                            .max_h_full()
                            .rounded(px(tokens::PAGE_RADIUS))
                            .shadow(tokens::page_shadow(palette().dark))
                            .with_animation(
                                "page-fade",
                                Animation::new(tokens::MEDIUM).with_easing(ease),
                                |panel, delta| panel.opacity(delta),
                            ),
                    )
                    .into_any_element(),
                // .popover (ui/styles.css:1010-1033), in the sidebar.
                Placement::SidebarFoot | Placement::SidebarTop(_) => {
                    let width = sidebar_width - tokens::POPOVER_LEFT - tokens::POPOVER_RIGHT;
                    div()
                        .absolute()
                        .left(px(tokens::POPOVER_LEFT))
                        .w(px(width))
                        .map(|frame| match placement {
                            Placement::SidebarTop(top) => frame.top(px(top)).bottom(px(0.)),
                            _ => frame
                                .bottom(px(tokens::POPOVER_BOTTOM))
                                .top_0()
                                .flex()
                                .flex_col()
                                .justify_end(),
                        })
                        .child(
                            div()
                                .max_h(relative(tokens::POPOVER_MAX_HEIGHT / 100.))
                                .min_h_0()
                                .flex()
                                .flex_col()
                                .child(panel.w_full())
                                .with_animation(
                                    "popover-rise",
                                    Animation::new(tokens::MEDIUM).with_easing(ease),
                                    |panel, delta| panel.opacity(delta).mt(px(6. * (1. - delta))),
                                ),
                        )
                        .into_any_element()
                }
            };
            main = main.child(deferred(
                div()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .when(placement == Placement::Palette, |backdrop| {
                        backdrop.bg(palette().backdrop)
                    })
                    .child(div().absolute().inset_0().on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _: &MouseDownEvent, window, cx| {
                            this.dismiss_overlay(window, cx);
                            cx.stop_propagation();
                        }),
                    ))
                    .child(frame),
            ));
        }
        if let Overlay::ContextMenu(menu) = &self.overlay {
            main = main.child(self.context_menu_panel(menu, window, cx));
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

/// Where an overlay sits, as the shipping shell places the same surface.
#[derive(Clone, Copy, PartialEq)]
enum Placement {
    /// The command bar, centred 16vh from the top over a dimmed window.
    Palette,
    /// A page of its own (Settings, History), over the page area.
    Page,
    /// A popover above the sidebar's footer (downloads).
    SidebarFoot,
    /// A popover this far from the sidebar's top (site controls, the Space editor).
    SidebarTop(f32),
}

/// A floating surface: `.popover` (ui/styles.css:1010-1022), and the command bar's box.
pub(crate) fn overlay_panel() -> Div {
    div()
        .flex()
        .flex_col()
        .min_h_0()
        .max_h_full()
        .occlude()
        .gap(px(tokens::SPACE_2))
        .p(px(tokens::SPACE_3))
        .rounded(px(tokens::RADIUS_LG))
        .bg(palette().surface)
        .text_color(palette().text)
        .shadow(palette().shadow_pop())
}

/// A text button on a floating surface: `.pill-button` (ui/styles.css:1190-1203), 28 tall.
pub(crate) fn button(
    id: impl Into<gpui::ElementId>,
    name: impl Into<gpui::SharedString>,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .px(px(tokens::PILL_PAD))
        // 28px tall around one line of 13px text at 1.35.
        .py(px((tokens::PILL_HEIGHT
            - tokens::TEXT_MD * tokens::LINE_HEIGHT)
            / 2.))
        .rounded(px(tokens::RADIUS_SM))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|style| style.bg(palette().surface_2))
        .child(name.into())
}

/// `.pill-button` with an icon before its label (the Peek bar's "Open as Tab").
pub(crate) fn pill_button(
    id: impl Into<gpui::ElementId>,
    name: impl Into<gpui::SharedString>,
    leading: Option<&'static str>,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(tokens::PILL_GAP))
        .h(px(tokens::PILL_HEIGHT))
        .px(px(tokens::PILL_PAD))
        .rounded(px(tokens::RADIUS_SM))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|style| style.bg(palette().surface_2))
        .children(leading.map(|name| icon(name, tokens::PILL_ICON, palette().text)))
        .child(name.into())
}

/// `.icon-button` (ui/styles.css:132-155): a 28px square in `--text-2` that fills on hover.
/// `label` names it for assistive technology.
pub(crate) fn icon_button(
    id: impl Into<gpui::ElementId>,
    name: &'static str,
    label: &'static str,
) -> gpui::Stateful<Div> {
    sized_icon_button(id, name, label, tokens::ICON_BUTTON, tokens::ICON)
}

/// `.icon-button` at another size: the sidebar toolbar's (ui/styles.css:65-73) or a row's close
/// button (ui/styles.css:629-650).
pub(crate) fn sized_icon_button(
    id: impl Into<gpui::ElementId>,
    name: &'static str,
    label: &'static str,
    size: f32,
    glyph: f32,
) -> gpui::Stateful<Div> {
    tinted_icon_button(id, name, label, (size, glyph), palette().text_2)
}

/// An icon button whose icon rests in `color` and turns `--text` on hover, as `.url-action`
/// does in `--text-3` (ui/styles.css:200-224).
pub(crate) fn tinted_icon_button(
    id: impl Into<gpui::ElementId>,
    name: &'static str,
    label: &'static str,
    (size, glyph): (f32, f32),
    color: gpui::Rgba,
) -> gpui::Stateful<Div> {
    let group = gpui::SharedString::new_static("icon-button");
    square(id, label, size)
        .group(group.clone())
        .cursor_pointer()
        .hover(|style| style.bg(palette().fill_hover))
        .child(
            icon(name, glyph, color).group_hover(group, |glyph| glyph.text_color(palette().text)),
        )
}

/// `.icon-button` with its glyph turned clockwise by `turn` (0 to 1) of a full circle, as an open folder turns its
/// chevron (ui/styles.css:772-774).
pub(crate) fn turned_icon_button(
    id: impl Into<gpui::ElementId>,
    name: &'static str,
    label: &'static str,
    turn: f32,
) -> gpui::Stateful<Div> {
    let group = gpui::SharedString::new_static("icon-button");
    square(id, label, tokens::ICON_BUTTON)
        .group(group.clone())
        .cursor_pointer()
        .hover(|style| style.bg(palette().fill_hover))
        .child(
            icon(name, tokens::ICON, palette().text_2)
                .with_transformation(gpui::Transformation::rotate(gpui::percentage(turn)))
                .group_hover(group, |glyph| glyph.text_color(palette().text)),
        )
}

/// An icon button's square without its hover fill, for a disabled one.
pub(crate) fn icon_square(
    id: impl Into<gpui::ElementId>,
    name: &'static str,
    label: &'static str,
    (size, glyph): (f32, f32),
    color: gpui::Rgba,
) -> gpui::Stateful<Div> {
    square(id, label, size).child(icon(name, glyph, color))
}

fn square(id: impl Into<gpui::ElementId>, label: &'static str, size: f32) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(tokens::RADIUS_SM))
        .aria_label(label)
}

/// `.text-button` (ui/styles.css:157-170): a small label in `--text-3` that fills on hover.
pub(crate) fn text_button(
    id: impl Into<gpui::ElementId>,
    name: impl Into<gpui::SharedString>,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .flex_none()
        .px(px(tokens::TEXT_BUTTON_PAD_X))
        .py(px(tokens::TEXT_BUTTON_PAD_Y))
        .rounded(px(tokens::RADIUS_SM))
        .text_size(px(tokens::TEXT_XS))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette().text_3)
        .cursor_pointer()
        .hover(|style| style.bg(palette().fill_hover).text_color(palette().text))
        .child(name.into())
}

/// `.primary-button` (ui/styles.css:1603-1626): ink on paper, inverted in Dark.
pub(crate) fn primary_button(
    id: impl Into<gpui::ElementId>,
    name: impl Into<gpui::SharedString>,
) -> gpui::Stateful<Div> {
    let palette = palette();
    let (ground, ink) = if palette.dark {
        (palette.paper, palette.ink)
    } else {
        (palette.ink, palette.paper)
    };
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .h(px(tokens::PRIMARY_BUTTON))
        .px(px(tokens::PRIMARY_BUTTON_PAD))
        .rounded(px(tokens::RADIUS_SM))
        .bg(ground)
        .text_color(ink)
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .child(name.into())
}

/// A page in the page area: rounded to `--page-radius`, with the frame's drop shadow, on the
/// empty state's colour until its first frame.
pub(crate) fn page_card(page: Div) -> Div {
    div()
        .size_full()
        .rounded(px(tokens::PAGE_RADIUS))
        .shadow(tokens::page_shadow(palette().dark))
        .bg(palette().page)
        .overflow_hidden()
        .child(page)
}

/// `inset 0 0 0 0.5px color`: the hairline ring the shipping shell draws with box-shadow.
pub(crate) fn hairline(color: gpui::Rgba) -> gpui::BoxShadow {
    gpui::BoxShadow {
        color: color.into(),
        offset: gpui::point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(0.5),
        inset: true,
    }
}

/// The red close circle of a window without the platform's title bar
/// (ui/windowControls.css:4-56): a 12px circle in a 20px target, its cross shown on hover.
pub(crate) fn window_close(id: &'static str) -> gpui::Stateful<Div> {
    let inset = tokens::WINDOW_CONTROL_INSET;
    let group = gpui::SharedString::new_static("window-close");
    div()
        .id(id)
        .group(group.clone())
        .relative()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(tokens::WINDOW_CONTROL))
        .cursor_pointer()
        .aria_label("Close window")
        .child(
            div()
                .absolute()
                .top(px(inset))
                .left(px(inset))
                .size(px(tokens::WINDOW_CONTROL - 2. * inset))
                .rounded_full()
                .bg(tokens::color(".window-control.close", false))
                .shadow(tokens::shadow_window_control()),
        )
        .child(
            icon(
                "window-close",
                tokens::WINDOW_CONTROL_GLYPH,
                tokens::color(".window-control svg stroke", false),
            )
            .opacity(0.)
            .group_hover(group, |glyph| glyph.opacity(1.)),
        )
}

/// `0 0 0 width color` outside an element: the focus ring.
pub(crate) fn ring(color: gpui::Rgba, width: f32) -> gpui::BoxShadow {
    gpui::BoxShadow {
        color: color.into(),
        offset: gpui::point(px(0.), px(0.)),
        blur_radius: px(0.),
        spread_radius: px(width),
        inset: false,
    }
}

/// `.shell-menu-item` (chromium/shell/shell.css:85-99): the pointer's item takes the selection
/// colour.
pub(crate) fn menu_item(
    id: impl Into<gpui::ElementId>,
    name: impl Into<gpui::SharedString>,
) -> gpui::Stateful<Div> {
    div()
        .id(id)
        .px(px(tokens::MENU_ITEM_PAD_X))
        .py(px(tokens::MENU_ITEM_PAD_Y))
        .rounded(px(tokens::MENU_ITEM_RADIUS))
        .whitespace_nowrap()
        .cursor_default()
        .hover(|item| {
            item.bg(palette().selection)
                .text_color(palette().selection_text)
        })
        .child(name.into())
}

/// `host()` in ui/app.ts:83-91: a URL's host without `www.`, or the text itself.
pub(crate) fn site(url: &str) -> &str {
    let Some((_, rest)) = url.split_once("://") else {
        return url;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    if host.is_empty() {
        return url;
    }
    host.strip_prefix("www.").unwrap_or(host)
}

/// `.favicon.letter` (ui/styles.css:741-756, ui/app.ts:112-115): the site's first letter on a
/// colour of its own, where the shipping shell would show the site's icon.
pub(crate) fn letter_favicon(url: &str, size: f32, radius: f32, text: f32) -> Div {
    let site = site(url);
    let letter = site
        .trim_start_matches(|c: char| !c.is_ascii_alphanumeric())
        .chars()
        .next()
        .map_or_else(|| "•".to_owned(), |c| c.to_uppercase().collect());
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(radius))
        .bg(Palette::letter(tokens::hue_for(site)))
        .text_color(gpui::white())
        .text_size(px(text))
        .line_height(relative(1.))
        .font_weight(FontWeight::BOLD)
        .child(letter)
}

/// `kbd` (ui/tokens.css:158-168): a key name in a small chip.
pub(crate) fn kbd(keys: impl Into<gpui::SharedString>) -> Div {
    div()
        .flex_none()
        .min_w(px(tokens::KBD_MIN_WIDTH))
        .px(px(tokens::KBD_PAD_X))
        .py(px(tokens::KBD_PAD_Y))
        .rounded(px(tokens::KBD_RADIUS))
        .bg(palette().fill_subtle)
        .shadow(vec![hairline(palette().surface_line)])
        .font_family(tokens::MONO)
        .font_weight(FontWeight::MEDIUM)
        .text_size(px(tokens::TEXT_XS))
        .line_height(px(tokens::KBD_LINE))
        .text_color(palette().text_2)
        .flex()
        .justify_center()
        .child(keys.into())
}

/// A `.switch`'s geometry: the shell's (ui/styles.css:1551-1584) or Settings' own
/// (ui/settings.css:323-356).
#[derive(Clone, Copy)]
pub(crate) struct SwitchSize {
    width: f32,
    height: f32,
    knob: f32,
    inset: f32,
    travel: f32,
}

pub(crate) const SHELL_SWITCH: SwitchSize = SwitchSize {
    width: tokens::SWITCH_WIDTH,
    height: tokens::SWITCH_HEIGHT,
    knob: tokens::SWITCH_KNOB,
    inset: tokens::SWITCH_KNOB_INSET,
    travel: tokens::SWITCH_TRAVEL,
};

pub(crate) const SETTINGS_SWITCH: SwitchSize = SwitchSize {
    width: tokens::SETTINGS_SWITCH_WIDTH,
    height: tokens::SETTINGS_SWITCH_HEIGHT,
    knob: tokens::SETTINGS_SWITCH_KNOB,
    inset: tokens::SETTINGS_SWITCH_KNOB_INSET,
    travel: tokens::SETTINGS_SWITCH_TRAVEL,
};

/// `.switch`: on in the selection colour with its knob slid across, off in `--surface-line`.
pub(crate) fn switch(
    id: impl Into<gpui::ElementId>,
    on: bool,
    label: &'static str,
    size: SwitchSize,
) -> gpui::Stateful<Div> {
    let palette = palette();
    div()
        .id(id)
        .relative()
        .flex_none()
        .w(px(size.width))
        .h(px(size.height))
        .rounded(px(size.height / 2.))
        .cursor_pointer()
        .aria_label(label)
        .map(|track| {
            if on {
                track.bg(palette.selection)
            } else {
                track
                    .bg(palette.surface_line)
                    .shadow(vec![hairline(palette.surface_line)])
            }
        })
        .child(
            div()
                .absolute()
                .top(px(size.inset))
                .left(px(size.inset + if on { size.travel } else { 0. }))
                .size(px(size.knob))
                .rounded_full()
                .bg(tokens::color(".switch::after background", false))
                .shadow(tokens::shadow_knob()),
        )
}

/// `.popover-footer` (ui/styles.css:1586-1592): a hairline above the popover's last actions.
/// `.popover-header` (ui/styles.css:1035-1053): a semibold title that truncates before the
/// trailing button does.
pub(crate) fn popover_header(title: impl Into<gpui::SharedString>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .pt(px(tokens::POPOVER_PAD_TOP))
        .pr(px(tokens::POPOVER_PAD_RIGHT))
        .pb(px(tokens::POPOVER_PAD_BOTTOM))
        .pl(px(tokens::POPOVER_PAD_LEFT))
        .font_weight(FontWeight::SEMIBOLD)
        .child(div().min_w_0().truncate().child(title.into()))
}

pub(crate) fn popover_footer() -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .pt(px(tokens::POPOVER_FOOTER_PAD_TOP))
        .px(px(tokens::POPOVER_FOOTER_PAD_X))
        .pb(px(tokens::POPOVER_FOOTER_PAD_BOTTOM))
        .border_t(px(tokens::POPOVER_RULE))
        .border_color(palette().surface_line)
}

/// `.text-button.danger` (ui/styles.css:1594-1601): a destructive action in red.
pub(crate) fn danger_button(
    id: impl Into<gpui::ElementId>,
    name: impl Into<gpui::SharedString>,
) -> gpui::Stateful<Div> {
    // Its red outranks `.text-button:hover`, which comes earlier in the file.
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .px(px(tokens::TEXT_BUTTON_PAD_X))
        .py(px(tokens::TEXT_BUTTON_PAD_Y))
        .rounded(px(tokens::RADIUS_SM))
        .text_size(px(tokens::TEXT_SM))
        .font_weight(FontWeight::MEDIUM)
        .text_color(palette().red)
        .cursor_pointer()
        .hover(|style| style.bg(palette().fill_hover))
        .child(name.into())
}

/// The first `--font` family installed here. Without one the shipping shell lands on
/// `sans-serif`, which Chromium on Linux draws in fontconfig's match for its default sans-serif
/// family, Arial; GPUI loads only named families, so this asks fontconfig the same question.
/// Without fontconfig, GPUI's own UI font.
fn ui_font(cx: &gpui::App) -> Option<gpui::SharedString> {
    static FONT: std::sync::OnceLock<Option<gpui::SharedString>> = std::sync::OnceLock::new();
    FONT.get_or_init(|| {
        let installed = cx.text_system().all_font_names();
        let is_installed = |name: &str| installed.iter().any(|family| family == name);
        if let Some(name) = tokens::FONT.iter().find(|name| is_installed(name)) {
            return Some(gpui::SharedString::new_static(name));
        }
        let matched = std::process::Command::new("fc-match")
            .args(["--format=%{family[0]}", tokens::LINUX_SANS_SERIF])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| String::from_utf8(output.stdout).ok())?;
        is_installed(&matched).then(|| matched.into())
    })
    .clone()
}

/// `isDevUrl` (ui/app.ts:611-619): a local or test host, which the address marks with a code
/// icon instead of a padlock.
pub(crate) fn is_dev_url(url: &str) -> bool {
    let host = site(url)
        .rsplit_once(':')
        .map_or(site(url), |(host, port)| {
            if port.chars().all(|c| c.is_ascii_digit()) {
                host
            } else {
                site(url)
            }
        });
    let host = host.to_ascii_lowercase();
    matches!(
        host.as_str(),
        "localhost" | "local" | "test" | "127.0.0.1" | "[::1]"
    ) || [".localhost", ".local", ".test"]
        .iter()
        .any(|suffix| host.ends_with(suffix))
}
