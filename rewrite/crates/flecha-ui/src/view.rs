use std::collections::BTreeMap;
use std::sync::Arc;

use flecha_cef::{Owner, Viewport};
use flecha_core::{
    TabId, WindowId,
    suggestions::{self, InputResolution, SuggestionAction},
};
use flecha_surface::{Frame, surface};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    App, AppContext as _, ClickEvent, ClipboardItem, Context, Div, Entity, FocusHandle,
    InteractiveElement as _, IntoElement, KeyDownEvent, ParentElement as _, Render, RenderImage,
    StatefulInteractiveElement as _, Styled as _, Window, canvas, div, px, relative, rgb,
};

use crate::{Action, Shared, dispatch, now};

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
    input: String,
    command: Command,
    images: BTreeMap<TabId, Vec<Arc<RenderImage>>>,
}

impl Shell {
    pub fn new(state: Shared, id: WindowId, window: &mut Window, cx: &mut App) -> Entity<Self> {
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        cx.new(|_| Self {
            state,
            id,
            focus,
            input: String::new(),
            command: Command::Open,
            images: BTreeMap::new(),
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
            let resolved = if local.is_none() {
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
                _ => return,
            }
        } else if stroke.key == "escape" {
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
            return;
        }
        cx.stop_propagation();
    }

    fn pane(
        &self,
        owner: Owner,
        id: TabId,
        images: &mut BTreeMap<TabId, Vec<Arc<RenderImage>>>,
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
        div()
            .relative()
            .size_full()
            .overflow_hidden()
            .bg(rgb(0x171e22))
            .child(surface(frames.view, frames.popup))
            .child(
                canvas(
                    |_, _, _| (),
                    move |pane, (), window, cx| {
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
                    },
                )
                .absolute()
                .size_full(),
            )
    }
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut main = div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x161c20))
            .text_color(rgb(0xe2edf1))
            .on_key_down(cx.listener(|this, event, window, cx| this.key(event, window, cx)));
        let mut images = BTreeMap::new();
        let state = self.state.borrow();
        if let Some(page) = state.model.flechinhas.get(&self.id) {
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
            commands =
                commands.child(
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
                            this.focus.focus(window, cx)
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
                let pane = self.pane(Owner::Tab(self.id.clone()), tab.clone(), &mut images);
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
                page_area = page_area
                    .child(div().absolute().size_full().bg(gpui::rgba(0x00000088)))
                    .child(
                        div()
                            .absolute()
                            .left(relative(0.08))
                            .right(relative(0.08))
                            .top(px(8.))
                            .bottom(px(24.))
                            .rounded(px(12.))
                            .overflow_hidden()
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
                            ))),
                    );
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
