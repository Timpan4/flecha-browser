use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

use flecha_cef::{BrowserKey, Owner, Viewport};
use flecha_core::{
    TabId, WindowId,
    suggestions::{self, InputResolution, SuggestionAction},
};
use flecha_surface::{Frame, input, surface};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, ClickEvent, ClipboardItem, Context, Div, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _, Pixels, Render, RenderImage,
    ScrollEvent, StatefulInteractiveElement as _, Styled as _, Subscription, Window, canvas,
    deferred, div, px, relative, rgb,
};

use crate::{Action, Shared, dispatch, now};

/// A popup's read-only address bar, as tall as the Peek toolbar.
pub(crate) const POPUP_BAR_HEIGHT: Pixels = px(40.);

#[derive(Clone, Copy)]
enum Command {
    Open,
    Split,
    Peek,
}

pub(crate) struct Shell {
    state: Shared,
    id: WindowId,
    focus: FocusHandle,
    page_focus: FocusHandle,
    selected_pane: Option<(Owner, TabId)>,
    focused_browser: Option<BrowserKey>,
    input: String,
    command: Command,
    images: BTreeMap<TabId, Vec<Arc<RenderImage>>>,
    title: String,
    _activation: Subscription,
}

impl Shell {
    pub fn new(state: Shared, id: WindowId, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let focus = cx.focus_handle();
        let page_focus = cx.focus_handle();
        focus.focus(window, cx);
        cx.new(|cx| {
            let activation = cx.observe_window_activation(window, |this: &mut Self, window, cx| {
                if window.is_window_active() {
                    this.state.borrow_mut().model.core.focus_window(&this.id);
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
                input: String::new(),
                command: Command::Open,
                images: BTreeMap::new(),
                title: String::new(),
                _activation: activation,
            }
        })
    }

    fn action_button(
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

    fn choose(&mut self, command: Command, window: &mut Window, cx: &mut Context<Self>) {
        self.command = command;
        self.focus.focus(window, cx);
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let input = std::mem::take(&mut self.input);
        let (local, resolved) = {
            let state = self.state.borrow();
            let core = &state.model.core;
            let current = state
                .model
                .active(&self.id)
                .and_then(|id| core.tab(id))
                .map(|tab| tab.url.as_str());
            let local = suggestions::developer_suggestions(
                &input,
                current,
                &core.settings.environments,
                now(),
            )
            .into_iter()
            .next()
            .map(|suggestion| suggestion.action);
            let private = suggestions::contains_private_jwt(&input);
            let local =
                local.filter(|action| !private || matches!(action, SuggestionAction::Copy(_)));
            let resolved = if local.is_none() && !private {
                suggestions::resolve_input(&input, core.settings.search_engine)
            } else {
                InputResolution::Invalid
            };
            (local, resolved)
        };
        let url = match (local, resolved) {
            (Some(SuggestionAction::Copy(text)), _) => {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                cx.notify();
                return;
            }
            (Some(SuggestionAction::Open(url)), _)
            | (_, InputResolution::Navigate(url) | InputResolution::Search(url)) => url,
            _ => {
                cx.notify();
                return;
            }
        };
        let action = match self.command {
            Command::Open => Action::Open(url),
            Command::Split => Action::SplitNew(url),
            Command::Peek => Action::Peek(url),
        };
        self.command = Command::Open;
        dispatch(&self.state, &self.id, action, cx);
    }

    fn key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let focused = self.sync_page_focus(window, cx);
        let stroke = &event.keystroke;
        let shortcut = if cfg!(target_os = "macos") {
            stroke.modifiers.platform
        } else {
            stroke.modifiers.control
        };
        if shortcut {
            match stroke.key.as_str() {
                "w" => dispatch(&self.state, &self.id, Action::Close, cx),
                "o" if self.state.borrow().model.flechinhas.contains_key(&self.id) => {
                    dispatch(&self.state, &self.id, Action::Return, cx);
                }
                "o" if self.state.borrow().model.peeks.contains_key(&self.id) => {
                    dispatch(&self.state, &self.id, Action::Promote, cx);
                }
                "l" | "t" => self.choose(Command::Open, window, cx),
                "q" => dispatch(&self.state, &self.id, Action::Quit, cx),
                "v" if self.focus.is_focused(window) => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        self.input.push_str(&text);
                        cx.notify();
                    }
                }
                "a" if self.focus.is_focused(window) => {
                    self.input.clear();
                    cx.notify();
                }
                _ => {
                    self.page_key(
                        focused,
                        stroke,
                        false,
                        event.is_held,
                        event.prefer_character_input,
                    );
                    return;
                }
            }
        } else if stroke.key == "escape" && self.state.borrow().model.peeks.contains_key(&self.id) {
            dispatch(&self.state, &self.id, Action::ClosePeek, cx);
            self.input.clear();
        } else if self.focus.is_focused(window) {
            match stroke.key.as_str() {
                "enter" => self.submit(cx),
                "backspace" => {
                    self.input.pop();
                    cx.notify();
                }
                _ => {
                    if let Some(text) = &stroke.key_char {
                        self.input.push_str(text);
                        cx.notify();
                    } else {
                        return;
                    }
                }
            }
        } else {
            self.page_key(
                focused,
                stroke,
                false,
                event.is_held,
                event.prefer_character_input,
            );
            return;
        }
        cx.stop_propagation();
    }

    fn sync_page_focus(&mut self, window: &mut Window, cx: &mut App) -> Option<BrowserKey> {
        let state = self.state.borrow();
        let pane = state.model.keyboard_pane(&self.id);
        let selection = pane
            .as_ref()
            .map(|(owner, tab)| (owner.clone(), (*tab).clone()));
        if self.selected_pane != selection {
            self.selected_pane = selection;
            if self.selected_pane.is_some() {
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

    fn page_key(
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
        let frames = self.state.borrow().engine.frames(&id).unwrap_or_default();
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
        element
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
            .track_focus(&self.page_focus)
            .on_key_down(cx.listener(|this, event, window, cx| this.key(event, window, cx)))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, window, cx| {
                let focused = this.sync_page_focus(window, cx);
                this.page_key(focused, &event.keystroke, true, false, false);
            }));
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
            let active_tab = state.model.active(&self.id);
            let view = current.visible_tabs();
            let mut sidebar = div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .flex_shrink_0()
                .w(px(core.settings.sidebar_width.pixels() as f32))
                .bg(rgb(0x202a30));
            for space in core.spaces() {
                let action = Action::Switch(space.id.clone());
                sidebar = sidebar.child(
                    button(format!("space-{}", space.id.as_str()), space.name.clone())
                        .when(&space.id == active_space, |row| row.bg(rgb(0x37545e)))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            dispatch(&this.state, &this.id, action.clone(), cx);
                        })),
                );
            }
            sidebar =
                sidebar.child(self.action_button("new-space", "New Space", Action::NewSpace, cx));
            if let Some(space) = core.space(active_space) {
                sidebar = sidebar.child(self.action_button(
                    "isolation",
                    if space.isolated {
                        "Separate cookies: On"
                    } else {
                        "Separate cookies: Off"
                    },
                    Action::Isolate,
                    cx,
                ));
            }
            for id in core.tab_ids(&self.id, active_space).unwrap_or_default() {
                let Some(tab) = core.tab(id) else { continue };
                let select = Action::Select(id.clone());
                let split = Action::Split(id.clone());
                let selected = active_tab == Some(id);
                sidebar = sidebar.child(
                    div()
                        .flex()
                        .gap_1()
                        .min_w_0()
                        .child(
                            button(
                                format!("tab-{}", id.as_str()),
                                tab.custom_title.as_ref().unwrap_or(&tab.title).clone(),
                            )
                            .truncate()
                            .flex_1()
                            .when(selected, |row| row.bg(rgb(0x37545e)))
                            .on_click(cx.listener(
                                move |this, _: &ClickEvent, _, cx| {
                                    dispatch(&this.state, &this.id, select.clone(), cx);
                                },
                            )),
                        )
                        .when(active_tab.is_some() && !selected, |row| {
                            row.child(button(format!("split-{}", id.as_str()), "Split").on_click(
                                cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    dispatch(&this.state, &this.id, split.clone(), cx);
                                }),
                            ))
                        }),
                );
            }
            let placeholder = match self.command {
                Command::Open => "Address or search",
                Command::Split => "Address to split",
                Command::Peek => "Address to peek",
            };
            let mut commands = div().flex().gap_2().p_2().items_center();
            for (label, mode) in [
                ("New tab", Command::Open),
                ("Split", Command::Split),
                ("Peek", Command::Peek),
            ] {
                commands = commands.child(button(label, label).on_click(cx.listener(
                    move |this, _: &ClickEvent, window, cx| this.choose(mode, window, cx),
                )));
            }
            commands = commands.child(
                div()
                    .id("command-input")
                    .track_focus(&self.focus)
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .rounded_md()
                    .px_2()
                    .py_1()
                    .bg(rgb(0x33434a))
                    .child(if self.input.is_empty() {
                        placeholder.to_owned()
                    } else {
                        self.input.clone()
                    })
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.choose(this.command, window, cx);
                    })),
            );
            if view.len() > 1 {
                commands = commands.child(self.action_button(
                    "unsplit",
                    "Exit Split",
                    Action::ExitSplit,
                    cx,
                ));
            }
            if active_tab.is_some() {
                commands = commands
                    .child(self.action_button("detach", "Flechinha", Action::Detach, cx))
                    .child(self.action_button("close-tab", "Close tab", Action::Close, cx));
            }
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
            let mut page_area = div().relative().flex_1().min_h_0().child(pages);
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
            main = main.child(commands).child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(sidebar)
                    .child(page_area),
            );
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

fn button(
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
