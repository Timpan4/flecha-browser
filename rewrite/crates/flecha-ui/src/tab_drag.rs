use flecha_core::{
    BrowserState, Device, FolderId, Placement, SpaceId, SplitDropTarget, SplitSide, Tab, TabId,
    WindowId,
};
use gpui::{
    App, AppContext as _, Context, Div, InteractiveElement as _, ParentElement as _, Pixels,
    Render, Stateful, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, relative, rgba,
};

use crate::icons::icon;
use crate::sidebar::tab_label;
use crate::theme::palette;
use crate::tokens;
use crate::view::Shell;
use crate::{Action, Controller, dispatch, now};

#[derive(Clone)]
pub(crate) struct TabDrag {
    window: WindowId,
    space: SpaceId,
    tab: TabId,
}

#[derive(Clone)]
pub(crate) enum TabZone {
    Today,
    Pinned(Option<FolderId>),
    Favorites,
}

#[derive(Clone)]
pub(crate) enum TabDrop {
    Place(TabZone, Option<TabId>),
    Split(SplitSide, SplitDropTarget),
}

#[derive(Clone, Copy)]
enum Cue {
    Insert,
    Section,
    RowSplit,
    PageSplit,
}

impl Controller {
    pub(crate) fn drop_tab(&mut self, window: &WindowId, drag: TabDrag, target: TabDrop) {
        let Some(core) = self.model.state_mut(window) else {
            return;
        };
        if !drag.is_in(window, core) {
            return;
        }
        match target {
            TabDrop::Place(zone, before) => {
                let placement = match &zone {
                    TabZone::Today => Placement::Today {
                        space: &drag.space,
                        window,
                    },
                    TabZone::Pinned(folder) => Placement::Pinned {
                        space: &drag.space,
                        folder: folder.as_ref(),
                    },
                    TabZone::Favorites => Placement::Favorite,
                };
                self.place_tab(window, &drag.tab, placement, before.as_ref());
            }
            TabDrop::Split(side, target) => {
                let change = core.drop_to_split(window, drag.tab, side, target, now());
                for removed in change.removed {
                    self.engine.close(&removed);
                }
            }
        }
    }
}

impl TabDrag {
    fn is_in(&self, window: &WindowId, core: &BrowserState) -> bool {
        &self.window == window
            && core
                .window(window)
                .is_some_and(|win| win.active_space_id == self.space)
            && match core.locate(&self.tab) {
                Some(Placement::Favorite) => true,
                Some(Placement::Pinned { space, .. }) => space == &self.space,
                Some(Placement::Today {
                    space,
                    window: owner,
                }) => space == &self.space && window == owner,
                None => false,
            }
    }
}

struct DragPreview {
    title: String,
    width: Pixels,
}

impl Render for DragPreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let colors = palette();
        div()
            .max_w(self.width)
            .px(px(tokens::SPACE_2))
            .h(px(tokens::ROW))
            .flex()
            .items_center()
            .rounded(px(tokens::RADIUS_MD))
            .bg(colors.surface)
            .text_color(colors.text)
            .shadow(colors.shadow_pop())
            .child(div().min_w_0().truncate().child(self.title.clone()))
    }
}

impl Shell {
    pub(crate) fn dragged_tab<'a>(&'a self, core: &BrowserState) -> Option<&'a TabId> {
        self.tab_drag
            .as_ref()
            .filter(|drag| drag.is_in(&self.id, core))
            .map(|drag| &drag.tab)
    }

    pub(crate) fn tab_drag_source(
        &self,
        element: Stateful<Div>,
        space: &SpaceId,
        id: &TabId,
        tab: &Tab,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        if tab.device == Some(Device::Mobile) {
            return element;
        }
        let shell = cx.entity().downgrade();
        let title = tab_label(tab);
        element.on_drag(
            TabDrag {
                window: self.id.clone(),
                space: space.clone(),
                tab: id.clone(),
            },
            move |drag, _, _, cx| {
                let width = shell
                    .update(cx, |shell, cx| {
                        shell.tab_drag = Some(drag.clone());
                        cx.notify();
                        px(shell
                            .state
                            .borrow()
                            .model
                            .core
                            .settings
                            .sidebar_width
                            .pixels() as f32)
                    })
                    .unwrap_or_default();
                cx.new(|_| DragPreview {
                    title: title.clone(),
                    width,
                })
            },
        )
    }

    pub(crate) fn tab_section_drop(&self, element: Div, zone: TabZone, cx: &Context<Self>) -> Div {
        self.tab_drop_zone(element, TabDrop::Place(zone, None), Cue::Section, cx)
    }

    pub(crate) fn tab_row_drop(
        &self,
        element: Div,
        zone: TabZone,
        source: &TabId,
        target: &TabId,
        can_split: bool,
        cx: &Context<Self>,
    ) -> Div {
        if source == target {
            return element;
        }
        let insert = self.tab_drop_zone(
            div().absolute().inset_0(),
            TabDrop::Place(zone, Some(target.clone())),
            Cue::Insert,
            cx,
        );
        element.relative().child(insert).when(can_split, |row| {
            // Same hit regions as Chromium's acceptDrops: 7px at each end for insertion.
            row.children([SplitSide::Left, SplitSide::Right].map(|side| {
                let region = div().absolute().top(px(7.)).bottom(px(7.)).occlude();
                let region = match side {
                    SplitSide::Left => region.left_0().w(relative(0.25)),
                    SplitSide::Right => region.right_0().w(relative(0.60)),
                };
                self.tab_drop_zone(
                    region,
                    TabDrop::Split(side, SplitDropTarget::Row(target.clone())),
                    Cue::RowSplit,
                    cx,
                )
            }))
        })
    }

    pub(crate) fn page_split_drop(&self, target: &TabId, cx: &Context<Self>) -> Div {
        let colors = palette();
        div()
            .absolute()
            .inset_0()
            .flex()
            .gap(px(tokens::PAGE_GAP))
            .children([SplitSide::Left, SplitSide::Right].map(|side| {
                let label = match side {
                    SplitSide::Left => "Open on the left",
                    SplitSide::Right => "Open on the right",
                };
                self.tab_drop_zone(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(tokens::PAGE_RADIUS))
                        .border(px(2.))
                        .border_color(rgba(0xffffff59))
                        .bg(rgba(0x0712212e))
                        .occlude()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(tokens::SPACE_2))
                                .px(px(tokens::SPACE_3))
                                .py(px(tokens::SPACE_2))
                                .rounded_full()
                                .bg(colors.surface)
                                .shadow(colors.shadow_pop())
                                .child(icon("columns", tokens::ICON, colors.text))
                                .child(label),
                        ),
                    TabDrop::Split(side, SplitDropTarget::Page(target.clone())),
                    Cue::PageSplit,
                    cx,
                )
            }))
    }

    fn tab_drop_zone(&self, element: Div, target: TabDrop, cue: Cue, cx: &Context<Self>) -> Div {
        let colors = palette();
        let element = match cue {
            Cue::Insert => element.border_t(px(2.)).border_color(rgba(0)),
            Cue::Section | Cue::RowSplit => element.rounded(px(tokens::RADIUS_MD)),
            Cue::PageSplit => element,
        };
        element
            .drag_over::<TabDrag>(move |style, _, _, _| match cue {
                Cue::Insert => style.border_color(colors.blue),
                Cue::Section => style.bg(colors.fill_hover),
                Cue::RowSplit => {
                    let mut fill = colors.blue;
                    fill.a *= 0.25;
                    style.bg(fill)
                }
                Cue::PageSplit => {
                    let mut fill = colors.selection;
                    fill.a *= 0.30;
                    style.bg(fill).border_color(colors.selection)
                }
            })
            .on_drop(
                cx.listener(move |shell, drag: &TabDrag, _, cx: &mut Context<Self>| {
                    dispatch(
                        &shell.state,
                        &shell.id,
                        Action::DropTab(drag.clone(), target.clone()),
                        cx,
                    );
                }),
            )
    }

    pub(crate) fn cancel_tab_drag(&mut self, window: &mut Window, cx: &mut App) -> bool {
        self.tab_drag.take().is_some() && cx.stop_active_drag(window)
    }
}

#[cfg(test)]
mod tests {
    use flecha_core::PinnedNode;

    use super::*;
    use crate::sidebar::SidebarAction;

    #[test]
    fn sidebar_drop_actions_reorder_and_persist_placement() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.core.focused_window().id.clone();
        let space = controller
            .model
            .core
            .focused_window()
            .active_space_id
            .clone();
        let mut open = |name| {
            controller
                .act(
                    &window,
                    Action::Open(format!("https://example.test/{name}")),
                )
                .unwrap();
            controller.model.active(&window).unwrap().clone()
        };
        let [a, b, c] = ["a", "b", "c"].map(&mut open);
        controller
            .act(
                &window,
                Action::Sidebar(SidebarAction::FolderName(None, "Reading".into())),
            )
            .unwrap();
        let PinnedNode::Folder { id: folder, .. } =
            &controller.model.core.space(&space).unwrap().pinned[0]
        else {
            panic!("expected a pinned folder");
        };
        let folder = TabZone::Pinned(Some(folder.clone()));
        let drop_on = |controller: &mut Controller, tab: &TabId, zone, before: Option<&TabId>| {
            let drag = TabDrag {
                window: window.clone(),
                space: space.clone(),
                tab: tab.clone(),
            };
            controller
                .act(
                    &window,
                    Action::DropTab(drag, TabDrop::Place(zone, before.cloned())),
                )
                .unwrap();
        };
        drop_on(&mut controller, &a, TabZone::Today, Some(&c));
        assert_eq!(
            controller.model.core.focused_window().spaces[&space].today,
            [a.clone(), c.clone(), b.clone()]
        );
        drop_on(&mut controller, &b, TabZone::Pinned(None), None);
        drop_on(&mut controller, &c, TabZone::Pinned(None), Some(&b));
        assert_eq!(
            controller.model.core.space(&space).unwrap().pinned_tabs(),
            [&c, &b]
        );
        drop_on(&mut controller, &b, folder.clone(), None);
        drop_on(&mut controller, &a, folder.clone(), Some(&b));
        assert_eq!(
            controller.model.core.space(&space).unwrap().pinned_tabs(),
            [&a, &b, &c]
        );
        drop_on(&mut controller, &a, TabZone::Favorites, None);
        drop_on(&mut controller, &b, TabZone::Favorites, Some(&a));
        assert_eq!(controller.model.core.favorites(), [b.clone(), a.clone()]);
        drop_on(&mut controller, &a, TabZone::Today, None);
        drop_on(&mut controller, &c, folder, None);
        drop(controller);

        let restarted = Controller::open_test_profile(profile.path()).unwrap();
        let core = &restarted.model.core;
        assert_eq!(core.favorites(), [b]);
        assert_eq!(core.focused_window().spaces[&space].today, [a]);
        assert!(matches!(
            core.space(&space).unwrap().pinned.as_slice(),
            [PinnedNode::Folder { children, .. }]
                if matches!(children.as_slice(), [PinnedNode::Tab { tab_id }] if tab_id == &c)
        ));
    }
}
