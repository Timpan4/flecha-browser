use std::time::{Duration, Instant};

use flecha_cef::ConnectionSecurity;
use flecha_core::suggestions::{ActiveEnvironment, EnvironmentTone, describe_environment};
use flecha_core::{
    BrowserState, Device, FolderId, PinnedKey, PinnedNode, Placement, SidebarWidth, SpaceId, TabId,
    WindowId, shortcuts,
};
use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, ClipboardItem, Context,
    DispatchPhase, Div, FontWeight, HoverListenerMode, InteractiveElement as _, IntoElement,
    MouseButton, MouseDownEvent, MouseExitEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, SharedString, StatefulInteractiveElement as _, Styled as _, Transformation, Window,
    canvas, div, percentage, prelude::FluentBuilder as _, px, relative,
};
use uuid::Uuid;

use crate::command_bar::Command;
use crate::empty_page::{live_dot, port_label};
use crate::icons::icon;
use crate::shell_menu::{self, MenuAnchor};
use crate::shortcuts::{Navigation, platform};
use crate::tab_drag::TabZone;
use crate::tab_menu::TabAction;
use crate::text_field::TextField;
use crate::theme::palette;
use crate::tokens;
use crate::view::{
    Overlay, Shell, hairline, icon_button, icon_square, is_dev_url, kbd, overlay_panel,
    primary_button, site, sized_icon_button, spinner, text_button, tinted_icon_button,
};
use crate::window_chrome::TitleBar;
use crate::{Action, Controller, dispatch, now};

#[derive(Clone)]
pub(crate) enum SidebarAction {
    Toggle,
    Pin(TabId, Option<FolderId>),
    Favorite(TabId),
    ToToday(TabId),
    FolderName(Option<FolderId>, String),
    ToggleFolder(FolderId, bool),
    RemoveFolder(FolderId),
    TogglePinned(bool),
    ClearToday,
    /// The agent folder's Clear: closes the tabs a coding agent opened in this Space.
    CloseAgentTabs,
}

/// The agent's folder in a Space (`agentTabs` in ui/app.ts:459): the Today tabs an agent opened.
fn agent_tabs(core: &BrowserState, space: Option<&flecha_core::WindowSpace>) -> Vec<TabId> {
    space.map_or_else(Vec::new, |space| {
        space
            .today
            .iter()
            .filter(|id| core.tab(id).is_some_and(|tab| tab.agent))
            .cloned()
            .collect()
    })
}

impl Controller {
    pub(crate) fn sidebar_action(&mut self, window: &WindowId, action: SidebarAction) {
        let Some(core) = self.model.state_mut(window) else {
            return;
        };
        let Some(current) = core.window(window) else {
            return;
        };
        let space = current.active_space_id.clone();
        match action {
            SidebarAction::Toggle => {
                core.set_sidebar_visible(window, !core.sidebar_visible(window));
            }
            SidebarAction::FolderName(Some(id), name) => {
                core.edit_folder(&id, Some(&name), None);
            }
            SidebarAction::FolderName(None, name) => {
                core.add_folder(
                    FolderId::new(Uuid::new_v4().to_string()),
                    &name,
                    &space,
                    None,
                );
            }
            SidebarAction::ToggleFolder(id, open) => {
                core.edit_folder(&id, None, Some(open));
            }
            SidebarAction::RemoveFolder(id) => {
                self.remove_pinned_folder(&id, true);
            }
            SidebarAction::TogglePinned(collapsed) => {
                core.update_space(&space, None, None, None, Some(collapsed));
            }
            SidebarAction::ClearToday => {
                for id in core.clear_today(window, now()) {
                    self.engine.close(&id);
                }
            }
            SidebarAction::CloseAgentTabs => {
                let tabs = agent_tabs(core, current.spaces.get(&space));
                for id in tabs {
                    self.close_tab(&id);
                }
            }
            SidebarAction::Pin(..) | SidebarAction::Favorite(_) | SidebarAction::ToToday(_) => {
                let (id, placement) = match &action {
                    SidebarAction::Pin(id, folder) => (
                        id,
                        Placement::Pinned {
                            space: &space,
                            folder: folder.as_ref(),
                        },
                    ),
                    SidebarAction::Favorite(id) => (id, Placement::Favorite),
                    SidebarAction::ToToday(id) => (
                        id,
                        Placement::Today {
                            space: &space,
                            window,
                        },
                    ),
                    _ => unreachable!(),
                };
                self.place_tab(window, id, placement, None);
            }
        }
    }

    pub(crate) fn place_tab(
        &mut self,
        window: &WindowId,
        id: &TabId,
        placement: Placement<'_>,
        before: Option<&TabId>,
    ) -> bool {
        let active = self.model.active(window) == Some(id)
            && match placement {
                Placement::Favorite => true,
                Placement::Pinned { space, .. } | Placement::Today { space, .. } => self
                    .model
                    .state(window)
                    .and_then(|state| state.window(window))
                    .is_some_and(|current| &current.active_space_id == space),
            };
        let Some(core) = self.model.state_mut(window) else {
            return false;
        };
        let change = core.place_tab(id, placement, before.map(PinnedKey::Tab));
        if change.result {
            for id in change.removed {
                self.engine.close(&id);
            }
            if active {
                for id in core.activate(id, window, Some(now())).removed {
                    self.engine.close(&id);
                }
            }
            self.reconcile_partition(id);
        }
        change.result
    }

    pub(crate) fn close_tab(&mut self, tab: &TabId) {
        self.engine.close(tab);
        for removed in self.model.close_tab(tab) {
            self.engine.close(&removed);
        }
    }

    pub(crate) fn remove_pinned_folder(&mut self, id: &FolderId, keep_tabs: bool) -> bool {
        let change = self.model.core.delete_folder(id, keep_tabs, now());
        for id in change.removed {
            self.engine.close(&id);
        }
        change.result
    }
}

// Shipping's #edge, --floating-sidebar-inset and peek animation (ui/styles.css).
const SIDEBAR_EDGE: f32 = 8.;
const FLOATING_INSET: f32 = 10.;
const PEEK_DELAY: Duration = Duration::from_millis(300);
const PEEK_IN: Duration = Duration::from_millis(180);
const PEEK_OUT: Duration = Duration::from_millis(160);

impl Controller {
    pub(crate) fn poll_sidebars(&self, cx: &mut App) {
        let now = Instant::now();
        for (id, deadline) in &self.sidebar_deadlines {
            if *deadline <= now
                && let Some(handle) = self.windows.get(id).copied()
            {
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |shell, _, cx| shell.advance_sidebar_hide(now, cx));
                });
            }
        }
    }
}

#[derive(Default)]
pub(crate) struct SidebarInteraction {
    peek: bool,
    leaving: bool,
    drag: Option<SidebarDrag>,
}

struct SidebarDrag {
    start_x: Pixels,
    start_width: SidebarWidth,
    width: SidebarWidth,
}

impl SidebarDrag {
    fn at(&self, x: Pixels) -> SidebarWidth {
        SidebarWidth::clamp(self.start_width.pixels() + f64::from(x - self.start_x))
            .unwrap_or(self.width)
    }
}

impl Shell {
    pub(crate) fn sidebar_width(&self, state: &Controller) -> f32 {
        self.sidebar_interaction
            .drag
            .as_ref()
            .map_or(state.model.core.settings.sidebar_width, |drag| drag.width)
            .pixels() as f32
    }

    pub(crate) fn sidebar_pointer(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.sidebar_interaction.drag.is_some() {
            return;
        }
        let state = self.state.borrow();
        let collapsed = state
            .model
            .state(&self.id)
            .is_some_and(|core| !core.sidebar_visible(&self.id));
        let fullscreen = state.fullscreen_pane(&self.id).is_some();
        let width = self.sidebar_width(&state);
        drop(state);
        if !collapsed || fullscreen {
            self.hide_floating_sidebar(cx);
            return;
        }
        let mouse = window.mouse_position();
        let within_height = mouse.y >= px(0.) && mouse.y < window.viewport_size().height;
        let hover_width = if self.sidebar_interaction.peek {
            width + FLOATING_INSET
        } else {
            SIDEBAR_EDGE
        };
        if within_height
            && mouse.x >= px(0.)
            && mouse.x < px(hover_width)
            && !self.overlay.is_open()
            && self.browser_dialog.is_none()
        {
            self.state.borrow_mut().sidebar_deadlines.remove(&self.id);
            if !self.sidebar_interaction.peek || self.sidebar_interaction.leaving {
                self.sidebar_interaction.peek = true;
                self.sidebar_interaction.leaving = false;
                cx.notify();
            }
        } else {
            self.leave_floating_sidebar();
        }
    }

    fn leave_floating_sidebar(&self) {
        if !self.sidebar_interaction.peek
            || self.sidebar_interaction.drag.is_some()
            || self.overlay.is_open()
            || self.browser_dialog.is_some()
        {
            return;
        }
        self.state
            .borrow_mut()
            .sidebar_deadlines
            .entry(self.id.clone())
            .or_insert_with(|| Instant::now() + PEEK_DELAY);
    }

    fn advance_sidebar_hide(&mut self, now: Instant, cx: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let Some(deadline) = state
            .sidebar_deadlines
            .get_mut(&self.id)
            .filter(|deadline| **deadline <= now)
        else {
            return;
        };
        if self.overlay.is_open()
            || self.browser_dialog.is_some()
            || self.sidebar_interaction.drag.is_some()
        {
            state.sidebar_deadlines.remove(&self.id);
            if std::mem::take(&mut self.sidebar_interaction.leaving) {
                cx.notify();
            }
        } else if self.sidebar_interaction.leaving {
            drop(state);
            self.hide_floating_sidebar(cx);
        } else {
            *deadline = now + PEEK_OUT;
            self.sidebar_interaction.leaving = true;
            cx.notify();
        }
    }

    pub(crate) fn hide_floating_sidebar(&mut self, cx: &mut Context<Self>) {
        let hover = &mut self.sidebar_interaction;
        hover.leaving = false;
        if std::mem::take(&mut hover.peek) {
            self.state.borrow_mut().sidebar_deadlines.remove(&self.id);
            cx.notify();
        }
    }

    pub(crate) fn floating_sidebar(
        &self,
        state: &Controller,
        current: &flecha_core::Window,
        title_bar: TitleBar,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        if !self.sidebar_interaction.peek {
            let palette = palette();
            let controls_room = (
                tokens::WINDOW_CONTROLS_WIDTH,
                tokens::WINDOW_CONTROLS_PAD_LEFT,
            );
            return div()
                .absolute()
                .top(px(tokens::SIDEBAR_PAD_TOP))
                .left(px(tokens::SIDEBAR_PAD_LEFT))
                .occlude()
                .flex()
                .items_center()
                .gap(px(tokens::TOOLBAR_GAP))
                .rounded(px(tokens::RADIUS_SM))
                .bg(palette.tint)
                .shadow(palette.shadow_row())
                .children(self.window_controls(title_bar, controls_room, window, cx))
                .child(self.toolbar_button(
                    "sidebar-toggle",
                    "sidebar",
                    "Show sidebar",
                    Action::Sidebar(SidebarAction::Toggle),
                    cx,
                ))
                .when(!state.model.peeks.contains_key(&self.id), |bar| {
                    bar.children(self.zoom_badge(cx))
                });
        }

        let mut float = div()
            .absolute()
            .left_0()
            .top_0()
            .bottom_0()
            .w(px(SIDEBAR_EDGE));
        if self.sidebar_interaction.peek {
            let slide = self.sidebar_width(state) + FLOATING_INSET;
            let leaving = self.sidebar_interaction.leaving;
            float = float.child(
                self.sidebar(state, current, title_bar, window, cx)
                    .absolute()
                    .left(px(FLOATING_INSET))
                    .top(px(FLOATING_INSET))
                    .bottom(px(FLOATING_INSET))
                    .rounded(px(tokens::WINDOW_RADIUS - FLOATING_INSET))
                    .shadow(palette().shadow_pop())
                    .occlude()
                    .with_animation(
                        if leaving {
                            "sidebar-slide-out"
                        } else {
                            "sidebar-slide-in"
                        },
                        Animation::new(if leaving { PEEK_OUT } else { PEEK_IN })
                            .with_easing(tokens::ease),
                        move |card, delta| {
                            card.ml(px(-slide * if leaving { delta } else { 1. - delta }))
                        },
                    ),
            );
        }
        float
    }

    pub(crate) fn sidebar_input(&self, cx: &Context<Self>) -> impl IntoElement {
        let shell = cx.entity();
        canvas(
            |_, _, _| (),
            move |_, (), window, _| {
                let view = shell.clone();
                window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                    if phase != DispatchPhase::Capture {
                        return;
                    }
                    view.update(cx, |this, cx| {
                        if let Some(drag) = &mut this.sidebar_interaction.drag {
                            if event.pressed_button == Some(MouseButton::Left) {
                                drag.width = drag.at(event.position.x);
                                cx.notify();
                            } else {
                                this.finish_sidebar_resize(Some(event.position.x), cx);
                            }
                            cx.stop_propagation();
                        } else {
                            this.sidebar_pointer(window, cx);
                        }
                    });
                });
                let view = shell.clone();
                window.on_mouse_event(move |_: &MouseExitEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture {
                        view.update(cx, |this, _| this.leave_floating_sidebar());
                    }
                });
                let view = shell.clone();
                window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                        view.update(cx, |this, cx| {
                            if this.sidebar_interaction.drag.is_some() {
                                this.finish_sidebar_resize(Some(event.position.x), cx);
                                this.sidebar_pointer(window, cx);
                                cx.stop_propagation();
                            }
                        });
                    }
                });
            },
        )
        .absolute()
        .size_0()
    }

    pub(crate) fn finish_sidebar_resize(&mut self, x: Option<Pixels>, cx: &mut Context<Self>) {
        if let Some(drag) = self.sidebar_interaction.drag.take() {
            let width = x.map_or(drag.width, |x| drag.at(x));
            if width != drag.start_width {
                dispatch(&self.state, &self.id, Action::ResizeSidebar(width), cx);
            }
            cx.notify();
        }
    }

    pub(crate) fn sidebar_drag_layer(&self) -> Option<Div> {
        self.sidebar_interaction
            .drag
            .as_ref()
            .map(|_| div().absolute().inset_0().occlude().cursor_col_resize())
    }

    /// `#sidebar` (ui/styles.css:25-36) and what it holds, in the shipping order: the toolbar,
    /// the address, Favorites, the Space's name, its pinned and Today tabs, and the footer.
    pub(crate) fn sidebar(
        &self,
        state: &Controller,
        current: &flecha_core::Window,
        title_bar: TitleBar,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let Some(core) = state.model.state(&self.id) else {
            return div();
        };
        let palette = palette();
        let incognito = state.model.is_incognito(&self.id);
        let toggle = Action::Sidebar(SidebarAction::Toggle);
        let docked = core.sidebar_visible(&self.id);
        let controls_room = (
            tokens::WINDOW_CONTROLS_WIDTH,
            tokens::WINDOW_CONTROLS_PAD_LEFT,
        );
        let Some(space) = core.space(&current.active_space_id) else {
            return div();
        };
        let window_space = current.spaces.get(&space.id);
        let active = window_space.and_then(|space| space.active_tab_id.as_ref());
        let split = current.visible_tabs();
        let toolbar = state.toolbars.get(&self.id).copied().unwrap_or_default();
        let rows = Rows {
            core,
            space: &space.id,
            active,
            split,
            dragging: self.dragged_tab(core),
            state,
            boosted: state.boosts.enabled().boosted_hosts(),
        };

        let mut tabs = div()
            .id("sidebar-tabs")
            .group("sidebar-tabs")
            .flex()
            .flex_col()
            .min_h_0()
            .flex_1()
            .overflow_y_scroll();
        if !incognito {
            let header = self.pinned_header(space, cx);
            tabs = tabs.child(if rows.dragging.is_some() {
                self.tab_section_drop(header, TabZone::Pinned(None), cx)
            } else {
                header
            });
            if !space.pinned_collapsed || rows.dragging.is_some() {
                tabs = tabs.children(self.pinned_rows(&rows, &space.pinned, None, cx));
                if rows.dragging.is_some() {
                    let end = div().flex_none().h(px(tokens::SPACE_1));
                    let end = if space.pinned.is_empty() {
                        end.h(px(tokens::ROW))
                            .flex()
                            .items_center()
                            .pl(px(tokens::ROW_PAD_LEFT))
                            .border_1()
                            .border_color(palette.line)
                            .text_color(palette.text_3)
                            .child("Drag tabs here to pin them")
                    } else {
                        end
                    };
                    tabs = tabs.child(self.tab_section_drop(end, TabZone::Pinned(None), cx));
                }
            }
        }
        if let Some(ws) = window_space {
            tabs = tabs.children(self.agent_folder(&rows, ws, cx));
        }
        let new_tab = self.new_tab_row(state, cx);
        tabs = tabs
            .child(
                // `.today-header` (ui/styles.css:475-481): a hairline above, a little apart.
                group_header()
                    .mt(px(tokens::SPACE_2))
                    .pt(px(tokens::SPACE_2))
                    .h(px(tokens::TODAY_HEADER))
                    .border_t(px(tokens::TODAY_RULE))
                    .border_color(palette.line)
                    .child("Today")
                    .child(
                        text_button("clear-today", "Clear")
                            .aria_label("Archive today's tabs")
                            .invisible()
                            .group_hover("sidebar-tabs", |clear| clear.visible())
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                dispatch(
                                    &this.state,
                                    &this.id,
                                    Action::Sidebar(SidebarAction::ClearToday),
                                    cx,
                                );
                            })),
                    ),
            )
            .child(if rows.dragging.is_some() {
                self.tab_section_drop(new_tab, TabZone::Today, cx)
            } else {
                new_tab
            });
        if let Some(ws) = window_space {
            tabs = tabs.children(self.today_rows(&rows, ws, cx));
        }
        if rows.dragging.is_some() {
            tabs = tabs.child(self.tab_section_drop(
                div().flex_none().h(px(tokens::SPACE_1)),
                TabZone::Today,
                cx,
            ));
        }

        let downloads = state
            .downloads
            .count(state.model.incognito_session(&self.id));
        div()
            .relative()
            .flex()
            .flex_col()
            .w(px(self.sidebar_width(state)))
            .flex_shrink_0()
            .min_h_0()
            .pt(px(tokens::SIDEBAR_PAD_TOP))
            .pr(px(tokens::SIDEBAR_PAD_RIGHT))
            .pb(px(tokens::SIDEBAR_PAD_BOTTOM))
            .pl(px(tokens::SIDEBAR_PAD_LEFT))
            .gap(px(tokens::SPACE_2))
            .bg(palette.tint)
            .child(
                self.window_drag(
                    // `.toolbar` (ui/styles.css:51-73, 105-127): the window controls, the sidebar
                    // toggle and the menu, then back, forward and reload, which wrap to a second
                    // line on the right in a sidebar too narrow for one. Its empty room moves the
                    // window.
                    div()
                        .flex()
                        .flex_wrap()
                        .flex_shrink_0()
                        .items_center()
                        .gap_x(px(tokens::TOOLBAR_GAP))
                        .gap_y(px(tokens::TOOLBAR_ROW_GAP))
                        .min_h(px(tokens::TOOLBAR_HEIGHT))
                        .children(self.window_controls(title_bar, controls_room, window, cx))
                        .child(
                            self.toolbar_button(
                                "sidebar-toggle",
                                "sidebar",
                                if docked {
                                    "Hide sidebar"
                                } else {
                                    "Pin sidebar"
                                },
                                toggle,
                                cx,
                            )
                            .occlude(),
                        )
                        .child(self.menu_button(incognito, cx).occlude())
                        .child(div().flex_1())
                        .child(
                            div()
                                .flex()
                                .flex_shrink_0()
                                .items_center()
                                .ml_auto()
                                .gap(px(tokens::TOOLBAR_GAP))
                                .occlude()
                                .when(!state.model.peeks.contains_key(&self.id), |nav| {
                                    nav.children(self.zoom_badge(cx))
                                })
                                .child(self.navigation_button(
                                    "back",
                                    "arrowLeft",
                                    Navigation::Back,
                                    toolbar.back,
                                    cx,
                                ))
                                .child(self.navigation_button(
                                    "forward",
                                    "arrowRight",
                                    Navigation::Forward,
                                    toolbar.forward,
                                    cx,
                                ))
                                .child(if toolbar.loading {
                                    self.navigation_button(
                                        "reload",
                                        "close",
                                        Navigation::Stop,
                                        true,
                                        cx,
                                    )
                                } else {
                                    self.navigation_button(
                                        "reload",
                                        "reload",
                                        Navigation::Reload,
                                        toolbar.available,
                                        cx,
                                    )
                                }),
                        ),
                    title_bar,
                    cx,
                ),
            )
            .child(
                div()
                    .id("sidebar-resize")
                    .aria_label("Resize sidebar")
                    .absolute()
                    .right_0()
                    .top_0()
                    .bottom_0()
                    .w(px(tokens::SIDEBAR_PAD_RIGHT))
                    .occlude()
                    .cursor_col_resize()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            let width = this.state.borrow().model.core.settings.sidebar_width;
                            this.state.borrow_mut().sidebar_deadlines.remove(&this.id);
                            this.sidebar_interaction.drag = Some(SidebarDrag {
                                start_x: event.position.x,
                                start_width: width,
                                width,
                            });
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .child(self.urlbar(state, &rows, toolbar.loading, cx))
            .children(agent_status(state))
            .when(!incognito, |sidebar| {
                sidebar.children(self.favorites(&rows, cx))
            })
            .children(self.local_servers(&rows, cx))
            .child(self.space_header(space, cx))
            .child(tabs)
            .child(
                // `footer` (ui/styles.css:917-922).
                div()
                    .flex()
                    .flex_shrink_0()
                    .items_center()
                    .gap(px(tokens::SPACE_1))
                    .pt(px(tokens::SPACE_1))
                    .child(
                        icon_button("show-downloads", "download", "Downloads")
                            .relative()
                            .when(downloads > 0, |button| {
                                button.child(downloads_badge(downloads))
                            })
                            .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.toggle_downloads(window, cx)
                            })),
                    )
                    .when(!incognito, |footer| {
                        footer.child(self.space_dots(core, space, cx)).child(
                            icon_button("new-space", "plus", "New Space").on_click(cx.listener(
                                |this, _: &ClickEvent, window, cx| {
                                    this.new_space(window, cx);
                                },
                            )),
                        )
                    }),
            )
    }

    /// `#urlbar-wrap` (ui/styles.css:174-276): the shown page's host behind a padlock, the site
    /// controls revealed on hover, and the loading line along its bottom.
    fn urlbar(
        &self,
        state: &Controller,
        rows: &Rows<'_>,
        loading: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let palette = palette();
        let url = rows
            .active
            .and_then(|id| rows.core.tab(id))
            .map(|tab| tab.url.as_str())
            .filter(|url| *url != "about:blank");
        let security = rows
            .active
            .zip(url)
            .map_or(ConnectionSecurity::Unknown, |(tab, url)| {
                state.engine.connection_security(tab, url)
            });
        let insecure_tls = matches!(
            security,
            ConnectionSecurity::CertificateError | ConnectionSecurity::InsecureContent
        );
        let insecure =
            insecure_tls || url.is_some_and(|url| url.starts_with("http:") && !is_dev_url(url));
        let kind = match url {
            None => "search",
            Some(_) if insecure_tls => "unlocked",
            Some(_) if security == ConnectionSecurity::VerifiedHttps => "lock",
            Some(url) if is_dev_url(url) => "code",
            Some(url) if url.starts_with("http:") => "unlocked",
            Some(_) => "globe",
        };
        let group = SharedString::new_static("urlbar");
        let flashing = state.copy_link_flashes.contains_key(&self.id);
        div()
            .id("urlbar-wrap")
            .group(group.clone())
            .relative()
            .flex()
            .flex_shrink_0()
            .items_center()
            .h(px(tokens::URLBAR_HEIGHT))
            .pr(px(tokens::URLBAR_PAD_RIGHT))
            .rounded(px(tokens::RADIUS_LG))
            .bg(palette.fill_subtle)
            .shadow(vec![hairline(palette.line)])
            .hover(|wrap| wrap.bg(palette.fill_hover))
            .child(
                div()
                    .id("sidebar-address")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .items_center()
                    .gap(px(tokens::SPACE_2))
                    .pl(px(tokens::URLBAR_PAD_LEFT))
                    .pr(px(tokens::URLBAR_PAD_RIGHT))
                    .cursor_pointer()
                    .aria_label("Open location")
                    .when(flashing, |address| address.opacity(0.))
                    .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                        this.open_command(Command::EditLocation, window, cx);
                    }))
                    .child(icon(
                        kind,
                        tokens::URLBAR_ICON,
                        if insecure {
                            palette.amber
                        } else {
                            palette.text_3
                        },
                    ))
                    .children(url.and_then(|url| {
                        describe_environment(url, &state.model.core.settings.environments)
                            .map(|environment| env_badge(&environment))
                    }))
                    .child(div().flex_1().min_w_0().truncate().map(|text| {
                        match url {
                            Some(url) => text
                                .font_weight(FontWeight::MEDIUM)
                                .child(site(url).to_owned()),
                            None => text.text_color(palette.text_3).child("Search or enter URL"),
                        }
                    })),
            )
            .when(rows.active.is_some() && !flashing, |wrap| {
                wrap.child(
                    tinted_icon_button(
                        "copy-link",
                        "link",
                        "Copy link",
                        (tokens::URL_ACTION, tokens::URL_ACTION_ICON),
                        palette.text_3,
                    )
                    .invisible()
                    .group_hover(group.clone(), |action| action.visible())
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.copy_link(cx))),
                )
            })
            .when(
                !flashing && state.site_for_window(&self.id).is_some(),
                |wrap| {
                    wrap.child(
                        tinted_icon_button(
                            "site-controls",
                            "sliders",
                            "Site controls",
                            (tokens::URL_ACTION, tokens::URL_ACTION_ICON),
                            palette.text_3,
                        )
                        .opacity(0.)
                        .group_hover(group, |action| action.opacity(1.))
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, window, cx| {
                                this.toggle_site_controls(window, cx)
                            },
                        )),
                    )
                },
            )
            .when(flashing, |wrap| {
                wrap.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("✓  Link copied"),
                )
            })
            .when_some(rows.active.filter(|_| loading), |wrap, tab| {
                wrap.child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .h(px(tokens::URLBAR_PROGRESS))
                        .bg(palette.blue)
                        .with_animation(
                            SharedString::from(format!("urlbar-progress-{}", tab.as_str())),
                            Animation::new(Duration::from_millis(tokens::PROGRESS_MS as u64)),
                            |line, delta| line.w(relative(tokens::progress(delta))),
                        ),
                )
            })
    }

    fn copy_link(&self, cx: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let url = state
            .model
            .active(&self.id)
            .and_then(|tab| state.model.tab(tab))
            .map(|tab| tab.url.clone());
        let Some(url) = url else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        // The shipping address bar shows the confirmation for 1400ms (ui/app.ts:986).
        state.copy_link_flashes.insert(
            self.id.clone(),
            Instant::now() + Duration::from_millis(1400),
        );
        cx.notify();
    }

    /// `#favorites` (ui/styles.css:414-457): a grid of four site tiles.
    fn favorites(&self, rows: &Rows<'_>, cx: &Context<Self>) -> Option<Div> {
        let favorites = rows.core.favorites();
        if favorites.is_empty() && rows.dragging.is_none() {
            return None;
        }
        let palette = palette();
        let mut grid = div()
            .id("favorites-strip")
            .grid()
            .grid_cols(4)
            .flex_shrink_0()
            .gap(px(tokens::FAVORITES_GAP));
        for id in favorites {
            let Some(tab) = rows.core.tab(id) else {
                continue;
            };
            let selected = rows.active == Some(id);
            let action = Action::Select(id.clone());
            let group = SharedString::from(format!("favorite-{}", id.as_str()));
            let tile = div()
                .id(group.clone())
                .flex()
                .items_center()
                .justify_center()
                .h(px(tokens::FAVORITE_HEIGHT))
                .rounded(px(tokens::RADIUS_LG))
                .cursor_pointer()
                .aria_label(tab_label(tab))
                .map(|tile| {
                    if selected {
                        tile.bg(palette.fill_active).shadow(palette.shadow_row())
                    } else {
                        tile.bg(palette.fill_subtle)
                            .hover(|tile| tile.bg(palette.fill_hover))
                    }
                })
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    dispatch(&this.state, &this.id, action.clone(), cx);
                }))
                .child(self.site_icon(
                    rows.state.tab_favicon(&self.id, id),
                    &tab.url,
                    (
                        tokens::FAVORITE_ICON,
                        tokens::FAVORITE_ICON_RADIUS,
                        tokens::FAVORITE_LETTER,
                    ),
                ));
            let tile = self
                .tab_pointer(div(), id, cx)
                .group(group.clone())
                .relative()
                .when(rows.dragging.is_some(), |tile| tile.occlude())
                .child(self.tab_drag_source(tile, rows.space, id, tab, cx))
                .when(selected, |tile| {
                    tile.child(
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .opacity(0.)
                            .group_hover(group, |menu| menu.opacity(1.))
                            .child(self.tab_menu_button(id, cx)),
                    )
                })
                .when_some(rows.dragging, |tile, source| {
                    self.tab_row_drop(tile, TabZone::Favorites, source, id, false, cx)
                });
            grid = grid.child(tile);
        }
        let strip = div().flex_shrink_0().child(grid);
        Some(if rows.dragging.is_some() {
            let strip = strip.when(favorites.is_empty(), |strip| {
                strip
                    .h(px(tokens::FAVORITE_HEIGHT))
                    .border_1()
                    .border_color(palette.line)
            });
            self.tab_section_drop(strip, TabZone::Favorites, cx)
        } else {
            strip
        })
    }

    /// The Pinned group's header (ui/index.html:72-75): its toggle and, in the rewrite, a way to
    /// make a folder that the shipping shell offers from its context menu.
    fn pinned_header(&self, space: &flecha_core::Space, cx: &Context<Self>) -> Div {
        let collapsed = space.pinned_collapsed;
        let action = Action::Sidebar(SidebarAction::TogglePinned(!collapsed));
        group_header()
            .child(
                text_button("pinned-toggle", "Pinned")
                    .aria_label(if collapsed {
                        "Show pinned tabs"
                    } else {
                        "Hide pinned tabs"
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        dispatch(&this.state, &this.id, action.clone(), cx);
                    })),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens::TOOLBAR_GAP))
                    // `#pinned-count` (ui/app.ts:243-244), while the group is folded away.
                    .when(collapsed && !space.pinned.is_empty(), |end| {
                        end.child(space.pinned.len().to_string())
                    })
                    .child(text_button("new-folder", "+ Folder").on_click(cx.listener(
                        |this, _: &ClickEvent, window, cx| {
                            this.show_overlay(
                                Overlay::Folder(None, TextField::default()),
                                window,
                                cx,
                            );
                        },
                    ))),
            )
    }

    /// The pinned tabs and folders; an open folder's contents sit indented past a hairline
    /// guide (`.folder-children`, ui/styles.css:776-790).
    fn pinned_rows(
        &self,
        rows: &Rows<'_>,
        nodes: &[PinnedNode],
        folder: Option<&FolderId>,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let palette = palette();
        nodes
            .iter()
            .map(|node| match node {
                PinnedNode::Tab { tab_id } => {
                    self.tab_row(rows, tab_id, TabZone::Pinned(folder.cloned()), false, cx)
                }
                PinnedNode::Folder {
                    id,
                    name,
                    open,
                    children,
                } => div()
                    .flex()
                    .flex_col()
                    .child(self.folder_row(rows, id, name, *open, cx))
                    .when(*open, |folder| {
                        folder.child(
                            div()
                                .relative()
                                .flex()
                                .flex_col()
                                .pl(px(tokens::FOLDER_INDENT))
                                .child(
                                    div()
                                        .absolute()
                                        .left(px(tokens::FOLDER_INDENT))
                                        .top(px(tokens::FOLDER_GUIDE_INSET))
                                        .bottom(px(tokens::FOLDER_GUIDE_INSET))
                                        .w(px(tokens::FOLDER_GUIDE))
                                        .bg(palette.line),
                                )
                                .children(self.pinned_rows(rows, children, Some(id), cx)),
                        )
                    })
                    .into_any_element(),
            })
            .collect()
    }

    /// A folder's row: its icon, name and a chevron that turns when it opens
    /// (ui/styles.css:760-774, ui/app.ts:431-448).
    fn folder_row(
        &self,
        rows: &Rows<'_>,
        id: &FolderId,
        name: &str,
        open: bool,
        cx: &Context<Self>,
    ) -> Div {
        let palette = palette();
        let toggle = Action::Sidebar(SidebarAction::ToggleFolder(id.clone(), !open));
        let rename = id.clone();
        let folder_name = name.to_owned();
        let group = SharedString::from(format!("folder-row-{}", id.as_str()));
        let chevron = icon("chevron", tokens::ICON, palette.text_3).when(open, |chevron| {
            chevron.with_transformation(Transformation::rotate(percentage(0.25)))
        });
        row(&group, false, None)
            .child(
                div()
                    .id(format!("folder-{}", id.as_str()))
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .items_center()
                    .gap(px(tokens::ROW_GAP))
                    .cursor_pointer()
                    .aria_label(if open { "Close folder" } else { "Open folder" })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        dispatch(&this.state, &this.id, toggle.clone(), cx);
                    }))
                    .child(row_icon(if open { "folderOpen" } else { "folder" }))
                    .child(div().min_w_0().truncate().child(name.to_owned()))
                    .child(
                        div()
                            .flex()
                            .ml_auto()
                            .opacity(0.)
                            .group_hover(group.clone(), |chevron| chevron.opacity(1.))
                            .child(chevron),
                    ),
            )
            .child(
                row_action(
                    &group,
                    sized_icon_button(
                        format!("rename-{}", id.as_str()),
                        "pencil",
                        "Rename folder",
                        tokens::ROW_CLOSE,
                        tokens::ROW_CLOSE_ICON,
                    ),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.show_overlay(
                        Overlay::Folder(
                            Some(rename.clone()),
                            TextField::new(folder_name.clone(), true),
                        ),
                        window,
                        cx,
                    );
                })),
            )
            .child(self.row_action(
                &group,
                "remove-folder",
                id.as_str(),
                ("close", "Remove folder"),
                Action::Sidebar(SidebarAction::RemoveFolder(id.clone())),
                cx,
            ))
            .when_some(rows.active, |row, active| {
                row.child(self.row_action(
                    &group,
                    "pin-in-folder",
                    id.as_str(),
                    ("plus", "Pin the shown tab here"),
                    Action::Sidebar(SidebarAction::Pin(active.clone(), Some(id.clone()))),
                    cx,
                ))
            })
            .when(rows.dragging.is_some(), |row| {
                self.tab_section_drop(row, TabZone::Pinned(Some(id.clone())), cx)
            })
    }

    fn today_rows(
        &self,
        rows: &Rows<'_>,
        space: &flecha_core::WindowSpace,
        cx: &Context<Self>,
    ) -> Vec<AnyElement> {
        let pair = space
            .split
            .as_ref()
            .filter(|pair| pair.iter().all(|id| space.today.contains(id)));
        let mut pair_drawn = false;
        space
            .today
            .iter()
            .filter_map(|id| {
                // The agent's tabs sit in its folder above Today.
                if rows.core.tab(id).is_some_and(|tab| tab.agent) {
                    return None;
                }
                if let Some(pair) = pair.filter(|pair| pair.contains(id)) {
                    if pair_drawn {
                        return None;
                    }
                    pair_drawn = true;
                    let palette = palette();
                    Some(
                        div()
                            .flex()
                            .gap(px(tokens::SPACE_1))
                            .p(px(tokens::ROW_MARGIN))
                            .rounded(px(tokens::RADIUS_MD))
                            .bg(if rows.split.len() == 2 {
                                palette.fill_hover
                            } else {
                                palette.fill_subtle
                            })
                            .children(
                                pair.iter()
                                    .map(|id| self.tab_row(rows, id, TabZone::Today, true, cx)),
                            )
                            .into_any_element(),
                    )
                } else {
                    Some(self.tab_row(rows, id, TabZone::Today, false, cx))
                }
            })
            .collect()
    }

    /// `.row` (ui/styles.css:578-713, ui/app.ts:543-590): the site's icon or a spinner while the
    /// page loads, the title, the audio mark, and the row's actions on hover in place of its
    /// Boost and split marks. A split pair leaves out the split mark (ui/styles.css:1955-1958).
    fn tab_row(
        &self,
        rows: &Rows<'_>,
        id: &TabId,
        zone: TabZone,
        paired: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let Some(tab) = rows.core.tab(id) else {
            return div().into_any_element();
        };
        let selected = rows.active == Some(id);
        let in_split = rows.split.len() > 1 && rows.split.contains(id);
        let hovered = rows.dragging.is_none() && self.hovered_tab_row.as_ref() == Some(id);
        let select = Action::Select(id.clone());
        let group = SharedString::from(format!("row-{}", id.as_str()));
        let engine = &rows.state.engine;
        let loading = engine.is_loading(id);
        let muted = tab.muted == Some(true);
        let boosted = rows
            .boosted
            .iter()
            .any(|host| *host != "all" && flecha_boosts::boost_matches(&tab.url, host));
        let palette = palette();
        let audio = (muted || engine.is_audible(id)).then(|| {
            let toggle = Action::Tab(id.clone(), TabAction::ToggleMute);
            tinted_icon_button(
                format!("audio-{}", id.as_str()),
                if muted { "muted" } else { "volume" },
                if muted { "Unmute" } else { "Mute" },
                (tokens::AUDIO_MARK, tokens::AUDIO_MARK_ICON),
                if muted {
                    palette.text_3
                } else {
                    palette.text_2
                },
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                dispatch(&this.state, &this.id, toggle.clone(), cx);
            }))
        });
        // A pair's actions float over the row's end on hover, so whatever ends the row makes
        // room for them.
        let actions_room = px(2. * tokens::ROW_CLOSE + tokens::SPACE_1);
        let pair_marks = paired && (audio.is_some() || boosted);
        let label = div()
            .id(format!("tab-{}", id.as_str()))
            .flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .items_center()
            .gap(px(if paired {
                tokens::SPACE_1
            } else {
                tokens::ROW_GAP
            }))
            .when(paired && hovered && !pair_marks, |label| {
                label.pr(actions_room)
            })
            .cursor_pointer()
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                dispatch(&this.state, &this.id, select.clone(), cx);
            }))
            .child(if loading {
                spinner(SharedString::from(format!("spinner-{}", id.as_str()))).into_any_element()
            } else {
                self.site_icon(
                    rows.state.tab_favicon(&self.id, id),
                    &tab.url,
                    (
                        tokens::FAVICON,
                        tokens::FAVICON_RADIUS,
                        tokens::FAVICON_LETTER,
                    ),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .when(selected, |title| title.font_weight(FontWeight::MEDIUM))
                    .when(loading, |title| title.text_color(palette.text_2))
                    .child(tab_label(tab)),
            );
        let row = self
            .tab_pointer(row(&group, selected, Some(in_split)), id, cx)
            .when(paired, |row| row.flex_1().my_0().relative())
            .child(self.tab_drag_source(label, rows.space, id, tab, cx));
        let row = if paired {
            row.when(pair_marks, |row| {
                row.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(tokens::SPACE_1))
                        .when(hovered, |marks| marks.pr(actions_room))
                        .children(audio)
                        .when(boosted && !hovered, |marks| {
                            marks.child(row_marks(true, false))
                        }),
                )
            })
        } else {
            row.children(audio)
                .when(!hovered && (boosted || in_split), |row| {
                    row.child(row_marks(boosted, in_split))
                })
        };
        row.when(hovered, |row| {
            let close = Action::CloseTab(id.clone());
            let close = sized_icon_button(
                format!("close-tab-{}", id.as_str()),
                "close",
                "Close tab",
                tokens::ROW_CLOSE,
                tokens::ROW_CLOSE_ICON,
            )
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                dispatch(&this.state, &this.id, close.clone(), cx);
            }));
            let menu = self.tab_menu_button(id, cx);
            if paired {
                return row.child(
                    div()
                        .absolute()
                        .right(px(tokens::ROW_PAD_RIGHT))
                        .top_0()
                        .bottom_0()
                        .flex()
                        .items_center()
                        .gap(px(tokens::SPACE_1))
                        .child(menu)
                        .child(close),
                );
            }
            row.child(menu).child(close)
        })
        .when_some(rows.dragging, |row, source| {
            self.tab_row_drop(
                row,
                zone,
                source,
                id,
                tab.device != Some(Device::Mobile),
                cx,
            )
        })
        .id(group)
        .hover_listener_mode(HoverListenerMode::InputModalityIndependent)
        .on_hover({
            let id = id.clone();
            cx.listener(move |this, hovered: &bool, _, cx| {
                if *hovered {
                    this.hovered_tab_row = Some(id.clone());
                    cx.notify();
                } else if this.hovered_tab_row.as_ref() == Some(&id) {
                    this.hovered_tab_row = None;
                    cx.notify();
                }
            })
        })
        .into_any_element()
    }

    /// `#btn-newtab` (ui/index.html:83, ui/styles.css:704-722): a row in `--text-2` with its
    /// shortcut shown on hover.
    fn new_tab_row(&self, state: &Controller, cx: &Context<Self>) -> Div {
        let group = SharedString::new_static("new-tab-row");
        let keys = shortcuts::display_keys(
            shortcuts::keys_for(&state.model.core.settings.keybindings, "newTab", platform()),
            platform(),
        );
        row(&group, false, None).text_color(palette().text_2).child(
            div()
                .id("new-tab")
                .flex()
                .flex_1()
                .min_w_0()
                .h_full()
                .items_center()
                .gap(px(tokens::ROW_GAP))
                .cursor_pointer()
                .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                    this.open_command(Command::NewTab, window, cx);
                }))
                .child(row_icon("plus"))
                .child(div().flex_1().child("New Tab"))
                .when(!keys.is_empty(), |row| {
                    row.child(
                        div()
                            .opacity(0.)
                            .group_hover(group, |keys| keys.opacity(1.))
                            .child(kbd(keys)),
                    )
                }),
        )
    }

    /// `#local-servers` (ui/index.html:59-62, ui/app.ts:393-412): dev servers on this machine.
    /// A click goes to the server's tab, or opens one.
    fn local_servers(&self, rows: &Rows<'_>, cx: &Context<Self>) -> Option<gpui::Stateful<Div>> {
        let servers = rows.state.local_servers();
        if servers.is_empty() {
            return None;
        }
        let palette = palette();
        let open_tabs = rows.core.open_tabs();
        let list = servers.iter().map(|server| {
            let origin = origin_of(&server.url);
            let open = open_tabs
                .iter()
                .find(|(_, tab)| origin_of(&tab.url) == origin)
                .map(|(id, _)| (*id).clone());
            let action = match &open {
                Some(id) => Action::Select(id.clone()),
                None => Action::Open(server.url.clone()),
            };
            let group = SharedString::from(format!("local-server-{}", server.port));
            row(&group, false, None)
                .h(px(tokens::LOCAL_SERVER_ROW))
                .child(
                    div()
                        .id(group.clone())
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .items_center()
                        .gap(px(tokens::ROW_GAP))
                        .cursor_pointer()
                        .aria_label(SharedString::from(server.url.clone()))
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            dispatch(&this.state, &this.id, action.clone(), cx);
                        }))
                        .child(live_dot())
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .when(open.is_some(), |title| {
                                    title.font_weight(FontWeight::MEDIUM)
                                })
                                .child(server.name.clone()),
                        )
                        .child(port_label(server.port)),
                )
        });
        Some(
            div()
                .id("local-servers")
                .flex()
                .flex_col()
                .flex_shrink_0()
                .child(group_header().child("Running locally"))
                .children(list)
                .text_color(palette.text),
        )
    }

    /// `#agent-folder` (ui/index.html:78, ui/app.ts:457-502): the tabs a coding agent opened,
    /// in a folder of their own above Today. The agent works in them in the background;
    /// dragging one out makes it an ordinary tab.
    fn agent_folder(
        &self,
        rows: &Rows<'_>,
        space: &flecha_core::WindowSpace,
        cx: &Context<Self>,
    ) -> Option<gpui::Stateful<Div>> {
        let tabs = agent_tabs(rows.core, Some(space));
        if tabs.is_empty() {
            return None;
        }
        let palette = palette();
        let busy = rows
            .state
            .agents
            .status(Instant::now())
            .is_some_and(|status| status.busy);
        let open = self.agent_folder_open;
        let group = SharedString::new_static("agent-folder");
        let chevron = icon("chevron", tokens::ICON, palette.text_3).when(open, |chevron| {
            chevron.with_transformation(Transformation::rotate(percentage(0.25)))
        });
        let header =
            row(&group, false, None)
                .child(
                    div()
                        .id("agent-folder-toggle")
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .items_center()
                        .gap(px(tokens::ROW_GAP))
                        .cursor_pointer()
                        .aria_label(if open {
                            "Close the agent's folder"
                        } else {
                            "Open the agent's folder"
                        })
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            this.agent_folder_open = !this.agent_folder_open;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .flex()
                                .flex_none()
                                .items_center()
                                .justify_center()
                                .w(px(tokens::FAVICON))
                                .map(|slot| {
                                    if busy {
                                        slot.child(agent_dot())
                                    } else {
                                        slot.child(icon("sparkle", tokens::ICON, palette.agent))
                                    }
                                }),
                        )
                        .child(div().min_w_0().truncate().text_color(palette.agent).child(
                            if busy {
                                "Agent is working…"
                            } else {
                                "Agent’s tabs"
                            },
                        ))
                        .child(
                            div()
                                .flex()
                                .ml_auto()
                                .opacity(0.)
                                .group_hover(group.clone(), |chevron| chevron.opacity(1.))
                                .child(chevron),
                        ),
                )
                .child(
                    text_button("clear-agent-tabs", "Clear")
                        .aria_label("Close the agent’s tabs")
                        .opacity(0.)
                        .group_hover(group, |clear| clear.opacity(1.))
                        .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                            dispatch(
                                &this.state,
                                &this.id,
                                Action::Sidebar(SidebarAction::CloseAgentTabs),
                                cx,
                            );
                        })),
                );
        let mut guide = palette.agent;
        guide.a *= tokens::AGENT_GUIDE_SHARE / 100.;
        Some(
            div()
                .id("agent-folder")
                .flex()
                .flex_col()
                .flex_none()
                .child(header)
                .when(open, |folder| {
                    folder.child(
                        div()
                            .relative()
                            .flex()
                            .flex_col()
                            .pl(px(tokens::FOLDER_INDENT))
                            .child(
                                div()
                                    .absolute()
                                    .left(px(tokens::FOLDER_INDENT))
                                    .top(px(tokens::FOLDER_GUIDE_INSET))
                                    .bottom(px(tokens::FOLDER_GUIDE_INSET))
                                    .w(px(tokens::FOLDER_GUIDE))
                                    .bg(guide),
                            )
                            .children(
                                tabs.iter()
                                    .map(|id| self.tab_row(rows, id, TabZone::Today, false, cx)),
                            ),
                    )
                }),
        )
    }

    /// The footer's Space buttons (ui/styles.css:924-990): each Space's emoji or initial, the
    /// shown one raised.
    fn space_dots(
        &self,
        core: &BrowserState,
        shown: &flecha_core::Space,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let palette = palette();
        let mut spaces = div()
            .id("spaces")
            .flex()
            .flex_1()
            .min_w_0()
            .justify_center()
            .gap(px(tokens::TOOLBAR_GAP))
            .overflow_x_scroll();
        for candidate in core.spaces() {
            let selected = candidate.id == shown.id;
            let action = Action::Switch(candidate.id.clone());
            let menu = candidate.id.clone();
            let label = match &candidate.emoji {
                Some(emoji) if !emoji.is_empty() => div()
                    .text_size(px(tokens::SPACE_EMOJI))
                    .child(emoji.clone()),
                _ => div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(tokens::SPACE_INITIAL))
                    .rounded(px(tokens::SPACE_INITIAL_RADIUS))
                    .bg(palette.text_2)
                    .text_color(palette.tint)
                    .text_size(px(tokens::SPACE_INITIAL_TEXT))
                    .font_weight(FontWeight::BOLD)
                    .child(
                        candidate
                            .name
                            .chars()
                            .next()
                            .map_or_else(|| "•".to_owned(), |c| c.to_uppercase().collect()),
                    ),
            };
            spaces = spaces.child(
                div()
                    .id(format!("space-{}", candidate.id.as_str()))
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .size(px(tokens::SPACE_DOT))
                    .rounded(px(tokens::RADIUS_SM))
                    .cursor_pointer()
                    .aria_label(candidate.name.clone())
                    .map(|dot| {
                        if selected {
                            dot.bg(palette.fill_active).shadow(palette.shadow_row())
                        } else {
                            dot.opacity(tokens::SPACE_DOT_IDLE_OPACITY)
                                .hover(|dot| dot.opacity(1.).bg(palette.fill_hover))
                        }
                    })
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        dispatch(&this.state, &this.id, action.clone(), cx);
                    }))
                    // `oncontextmenu` on `.space-dot` (ui/app.ts:215-218).
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            this.open_space_menu(&menu, event.position, window, cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(label),
            );
        }
        spaces
    }

    /// `#btn-menu` (ui/index.html): the window menu, which holds what the macOS menu bar does
    /// (ui/app.ts `windowMenu`). It drops from where the button was last drawn.
    fn menu_button(&self, incognito: bool, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let bounds = self.menu_button.clone();
        sized_icon_button(
            "window-menu",
            "menu",
            "Menu",
            tokens::TOOLBAR_BUTTON,
            tokens::TOOLBAR_ICON,
        )
        .relative()
        .child(
            canvas(move |drawn, _, _| bounds.set(drawn), |_, (), _, _| {})
                .absolute()
                .size_full(),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
            let entries = shell_menu::window_entries(
                incognito,
                &this.state.borrow().model.core.settings.keybindings,
            );
            let below = MenuAnchor::Below(this.menu_button.get());
            this.open_menu(entries, below, window, cx);
        }))
    }

    /// A sidebar toolbar button (ui/styles.css:65-73): 30px, its icon 18px.
    fn toolbar_button(
        &self,
        id: &'static str,
        name: &'static str,
        label: &'static str,
        action: Action,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        sized_icon_button(
            id,
            name,
            label,
            tokens::TOOLBAR_BUTTON,
            tokens::TOOLBAR_ICON,
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            dispatch(&this.state, &this.id, action.clone(), cx);
        }))
    }

    fn navigation_button(
        &self,
        id: &'static str,
        name: &'static str,
        navigation: Navigation,
        enabled: bool,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let label = match navigation {
            Navigation::Back => "Back",
            Navigation::Forward => "Forward",
            Navigation::Reload => "Reload",
            Navigation::HardReload => "Reload without cache",
            Navigation::Stop => "Stop",
        };
        if !enabled {
            // `.icon-button:disabled` (ui/styles.css:152-155).
            return icon_square(
                id,
                name,
                label,
                (tokens::TOOLBAR_BUTTON, tokens::TOOLBAR_ICON),
                palette().text_3,
            )
            .opacity(tokens::ICON_BUTTON_DISABLED_OPACITY);
        }
        sized_icon_button(
            id,
            name,
            label,
            tokens::TOOLBAR_BUTTON,
            tokens::TOOLBAR_ICON,
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            dispatch(&this.state, &this.id, Action::Browser(navigation), cx)
        }))
    }

    /// A row's icon button, shown while the pointer is over its row.
    fn row_action(
        &self,
        group: &SharedString,
        kind: &'static str,
        id: &str,
        (name, label): (&'static str, &'static str),
        action: Action,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        row_action(
            group,
            sized_icon_button(
                format!("{kind}-{id}"),
                name,
                label,
                tokens::ROW_CLOSE,
                tokens::ROW_CLOSE_ICON,
            ),
        )
        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
            dispatch(&this.state, &this.id, action.clone(), cx);
        }))
    }

    /// `#space-header` (ui/app.ts:1862-1866): a double click opens the Space editor, a right
    /// click the Space's menu.
    fn space_header(&self, space: &flecha_core::Space, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let id = space.id.clone();
        let menu = space.id.clone();
        space_title(space)
            .id("space-header")
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                if event.click_count() >= 2 {
                    this.open_space_editor(&id, window, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.open_space_menu(&menu, event.position, window, cx);
                    cx.stop_propagation();
                }),
            )
    }

    /// The row's menu button opens the tab's menu (`tabMenu`) where it was clicked.
    fn tab_menu_button(&self, id: &TabId, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let tab = id.clone();
        sized_icon_button(
            format!("tab-actions-{}", id.as_str()),
            "menu",
            "Tab actions",
            tokens::ROW_CLOSE,
            tokens::ROW_CLOSE_ICON,
        )
        .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
            this.open_tab_menu(&tab, event.position(), window, cx);
        }))
    }

    /// `wireTab` (ui/app.ts:688-695): a right click opens the tab's menu at the pointer, and a
    /// middle click closes the tab, as `auxclick` does on release.
    fn tab_pointer(&self, element: Div, id: &TabId, cx: &Context<Self>) -> Div {
        let menu = id.clone();
        let close = Action::CloseTab(id.clone());
        element
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    this.open_tab_menu(&menu, event.position, window, cx);
                    cx.stop_propagation();
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(move |this, _: &MouseUpEvent, _, cx| {
                    dispatch(&this.state, &this.id, close.clone(), cx);
                    cx.stop_propagation();
                }),
            )
    }

    pub(crate) fn folder_key(
        &mut self,
        stroke: &gpui::Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if stroke.key == "enter" && stroke.modifiers == gpui::Modifiers::none() {
            self.save_folder(window, cx);
        } else if let Overlay::Folder(_, input) = &mut self.overlay {
            input.key(stroke, cx);
            cx.notify();
        }
    }

    fn save_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Overlay::Folder(folder, input) = &self.overlay else {
            return;
        };
        let action = SidebarAction::FolderName(folder.clone(), input.text().to_owned());
        self.dismiss_overlay(window, cx);
        dispatch(&self.state, &self.id, Action::Sidebar(action), cx);
    }

    pub(crate) fn folder_panel(
        &self,
        id: Option<&FolderId>,
        input: &TextField,
        cx: &Context<Self>,
    ) -> Div {
        overlay_panel()
            .child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(if id.is_some() {
                        "Rename folder"
                    } else {
                        "New folder"
                    }),
            )
            .child(input.render("Folder name"))
            .child(div().flex().justify_end().child(
                primary_button("save-folder", "Save").on_click(cx.listener(
                    |this, _: &ClickEvent, window, cx| {
                        this.save_folder(window, cx);
                    },
                )),
            ))
    }
}

/// What every row needs to know about the window it sits in.
struct Rows<'a> {
    core: &'a BrowserState,
    space: &'a SpaceId,
    active: Option<&'a TabId>,
    split: &'a [TabId],
    dragging: Option<&'a TabId>,
    state: &'a Controller,
    /// `boostedHosts`: hosts with an enabled Boost.
    boosted: Vec<&'a str>,
}

/// `.boost-mark` and `.split-badge` (ui/styles.css:652-682, ui/app.ts:575-581): an amber bolt on
/// a Boosted site and a split mark on a tab in split view. The row's actions replace them on
/// hover.
fn row_marks(boosted: bool, in_split: bool) -> Div {
    let palette = palette();
    div()
        .flex()
        .items_center()
        .gap(px(tokens::ROW_GAP))
        .when(boosted, |marks| {
            marks.child(icon("zap", tokens::ROW_BADGE_ICON, palette.amber))
        })
        .when(in_split, |marks| {
            marks.child(icon("columns", tokens::ROW_BADGE_ICON, palette.text_3))
        })
}

/// `.row` (ui/styles.css:578-603): 32 tall, raised while it is the shown tab, tinted while it
/// shares the page area with it. `in_split` is `None` for rows that are not tabs.
fn row(group: &SharedString, selected: bool, in_split: Option<bool>) -> Div {
    let palette = palette();
    div()
        .group(group.clone())
        .flex()
        .flex_none()
        .items_center()
        .gap(px(tokens::ROW_GAP))
        .min_w_0()
        .h(px(tokens::ROW))
        .my(px(tokens::ROW_MARGIN))
        .pl(px(tokens::ROW_PAD_LEFT))
        .pr(px(tokens::ROW_PAD_RIGHT))
        .rounded(px(tokens::RADIUS_MD))
        .map(|row| {
            if selected {
                row.bg(palette.fill_active).shadow(palette.shadow_row())
            } else {
                let row = if in_split == Some(true) {
                    let mut fill = palette.fill_active;
                    fill.a *= tokens::IN_SPLIT_SHARE / 100.;
                    row.bg(fill)
                } else {
                    row
                };
                row.hover(|row| row.bg(palette.fill_hover))
            }
        })
}

/// `#agent-status` (ui/index.html:52-55, ui/styles.css:367-396, ui/app.ts:386-391): shown
/// while a coding agent is connected, with what it is doing right now.
fn agent_status(state: &Controller) -> Option<Div> {
    let status = state.agents.status(Instant::now())?;
    let palette = palette();
    let mut fill = palette.agent;
    fill.a *= tokens::AGENT_STATUS_SHARE / 100.;
    Some(
        div()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap(px(tokens::SPACE_2))
            .h(px(tokens::AGENT_STATUS))
            .px(px(tokens::AGENT_STATUS_PAD))
            .rounded(px(tokens::RADIUS_MD))
            .bg(fill)
            .text_color(palette.agent)
            .text_size(px(tokens::TEXT_SM))
            .font_weight(FontWeight::MEDIUM)
            .child(agent_dot())
            .child(div().min_w_0().truncate().child(if status.busy {
                format!("Agent is {}", status.action)
            } else {
                "Agent connected".to_owned()
            })),
    )
}

/// `.env-badge` (ui/styles.css:300-320, ui/app.ts:315-317): the project's label for the
/// environment the page is on, green for local, amber for staging and red for production.
fn env_badge(environment: &ActiveEnvironment) -> Div {
    let palette = palette();
    div()
        .flex_none()
        .py(px(tokens::ENV_BADGE_PAD_Y))
        .px(px(tokens::ENV_BADGE_PAD_X))
        .rounded(px(tokens::ENV_BADGE_RADIUS))
        .bg(match environment.tone {
            EnvironmentTone::Local => palette.green,
            EnvironmentTone::Staging => palette.amber,
            EnvironmentTone::Production => palette.red,
        })
        .text_color(tokens::color(".env-badge color", palette.dark))
        .font_family(tokens::MONO)
        .font_weight(FontWeight::BOLD)
        .text_size(px(tokens::ENV_BADGE_TEXT))
        .line_height(px(tokens::ENV_BADGE_LINE))
        .child(environment.label.to_uppercase())
}

/// `.agent-dot` (ui/styles.css:390-396).
fn agent_dot() -> Div {
    div()
        .flex_none()
        .size(px(tokens::AGENT_DOT))
        .rounded_full()
        .bg(palette().agent)
}

/// `originOf` (ui/app.ts:414-421): a page's origin, with 127.0.0.1 and localhost the same.
fn origin_of(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("", url));
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        host.to_ascii_lowercase()
    )
    .replace("127.0.0.1", "localhost")
}

/// `.row-icon` (ui/styles.css:620-625): a 16px column in `--text-2`.
fn row_icon(name: &'static str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(tokens::FAVICON))
        .child(icon(name, tokens::ICON, palette().text_2))
}

/// Hidden until the pointer is over the row, and keeping its place meanwhile so the controls
/// beside it stay put.
fn row_action(group: &SharedString, button: gpui::Stateful<Div>) -> gpui::Stateful<Div> {
    button
        .opacity(0.)
        .group_hover(group.clone(), |action| action.opacity(1.))
}

/// `.group-header` (ui/styles.css:466-474).
pub(crate) fn group_header() -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_between()
        .h(px(tokens::GROUP_HEADER))
        .pl(px(tokens::GROUP_HEADER_PAD_LEFT))
        .pr(px(tokens::GROUP_HEADER_PAD_RIGHT))
        .text_size(px(tokens::TEXT_XS))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette().text_3)
}

/// `#space-header` (ui/styles.css:493-542): the Space's emoji and name, and a padlock when it
/// keeps its own cookies.
fn space_title(space: &flecha_core::Space) -> Div {
    let palette = palette();
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(tokens::SPACE_HEADER_GAP))
        .h(px(tokens::SPACE_HEADER))
        .px(px(tokens::SPACE_HEADER_PAD))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.text)
        .children(
            space
                .emoji
                .as_ref()
                .filter(|emoji| !emoji.is_empty())
                .map(|emoji| {
                    div()
                        .text_size(px(tokens::SPACE_EMOJI))
                        .child(emoji.clone())
                }),
        )
        .child(div().min_w_0().truncate().child(space.name.clone()))
        .when(space.isolated, |header| {
            header.child(div().flex().child(icon(
                "lock",
                tokens::SPACE_ISOLATED_ICON,
                palette.text_3,
            )))
        })
}

/// `#downloads-badge` (ui/styles.css:991-1002): how many downloads are running, on the
/// Downloads button's corner.
fn downloads_badge(count: usize) -> Div {
    let palette = palette();
    div()
        .absolute()
        .top_0()
        .right_0()
        .min_w(px(tokens::DOWNLOADS_BADGE))
        .h(px(tokens::DOWNLOADS_BADGE))
        .px(px(tokens::DOWNLOADS_BADGE_PAD))
        .rounded(px(tokens::DOWNLOADS_BADGE_RADIUS))
        .bg(palette.selection)
        .text_color(gpui::white())
        .font_family(tokens::MONO)
        .font_weight(FontWeight::BOLD)
        .text_size(px(tokens::DOWNLOADS_BADGE_TEXT))
        .line_height(px(tokens::DOWNLOADS_BADGE))
        .flex()
        .justify_center()
        .child(count.to_string())
}

pub(crate) fn tab_label(tab: &flecha_core::Tab) -> String {
    let title = tab.custom_title.as_ref().unwrap_or(&tab.title);
    if title.is_empty() || *title == tab.url {
        flecha_core::suggestions::display_url(&tab.url)
    } else {
        title.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sidebar_actions_persist_space_and_tab_placement_changes() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        let first = controller
            .model
            .core
            .focused_window()
            .active_space_id
            .clone();
        let act = |controller: &mut Controller, action| controller.act(&window, action).unwrap();
        act(&mut controller, Action::NewSpace);
        let second = controller
            .model
            .core
            .focused_window()
            .active_space_id
            .clone();
        assert_ne!(first, second);
        act(
            &mut controller,
            Action::Open("https://example.test/".into()),
        );
        let tab = controller.model.active(&window).unwrap().clone();
        let sidebar =
            |controller: &mut Controller, action| act(controller, Action::Sidebar(action));
        sidebar(
            &mut controller,
            SidebarAction::FolderName(None, "Work".into()),
        );
        let PinnedNode::Folder { id, .. } =
            &controller.model.core.space(&second).unwrap().pinned[0]
        else {
            panic!("the Space needs a pinned folder")
        };
        let folder = id.clone();
        sidebar(
            &mut controller,
            SidebarAction::Pin(tab.clone(), Some(folder.clone())),
        );
        sidebar(
            &mut controller,
            SidebarAction::ToggleFolder(folder.clone(), false),
        );
        assert!(
            matches!(&controller.model.core.space(&second).unwrap().pinned[0],
            PinnedNode::Folder { open: false, children, .. }
                if matches!(&children[0], PinnedNode::Tab { tab_id } if tab_id == &tab))
        );
        sidebar(&mut controller, SidebarAction::ToToday(tab.clone()));
        assert!(
            matches!(controller.model.core.locate(&tab), Some(Placement::Today { space, .. }) if space == &second)
        );
        sidebar(&mut controller, SidebarAction::Favorite(tab.clone()));
        act(&mut controller, Action::Switch(first.clone()));
        assert_eq!(
            controller.model.core.focused_window().active_space_id,
            first
        );
        assert_eq!(
            controller.model.core.favorites(),
            std::slice::from_ref(&tab)
        );
        sidebar(&mut controller, SidebarAction::ToToday(tab.clone()));
        act(&mut controller, Action::CloseTab(tab));
        let entry = controller.model.core.archive()[0].clone();
        act(&mut controller, Action::RestoreArchive(entry));
        let restored = controller.model.active(&window).unwrap();
        assert_eq!(
            controller.model.core.tab(restored).unwrap().url,
            "https://example.test/"
        );
        assert!(controller.model.core.archive().is_empty());
        let visible = controller.model.core.sidebar_visible(&window);
        sidebar(&mut controller, SidebarAction::Toggle);
        assert_ne!(controller.model.core.sidebar_visible(&window), visible);
    }
}
