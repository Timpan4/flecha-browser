use anyhow::{Context as _, Result};
use flecha_cef::{
    BrowserKey, ClipboardEdit, ContextMenuEvent, ContextMenuRequest, Disposition, FrameEdit,
    MenuContext, Owner,
};
use flecha_core::{TabId, WindowId, suggestions::pinnable_address};
use gpui::{
    App, ClickEvent, ClipboardItem, Context, InteractiveElement as _, IntoElement, MouseDownEvent,
    ParentElement as _, Pixels, StatefulInteractiveElement as _, Styled as _, Window, deferred,
    div, px,
};

use crate::clipboard::Held;
use crate::model::tab_id;
use crate::popup::{Place, place};
use crate::shortcuts::Navigation;
use crate::theme::palette;
use crate::view::{Overlay, Shell, button};
use crate::{Action, Controller, dispatch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Destination {
    Tab,
    Peek,
    Incognito,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    Link(Destination),
    CopyLink,
    Image,
    CopyImage,
    Edit(FrameEdit),
    Navigate(Navigation),
    Inspect,
}

impl Item {
    fn label(self) -> &'static str {
        match self {
            Self::Link(Destination::Tab) => "Open link in new tab",
            Self::Link(Destination::Peek) => "Peek link",
            Self::Link(Destination::Incognito) => "Open link in Incognito",
            Self::CopyLink => "Copy link address",
            Self::Image => "Open image in new tab",
            Self::CopyImage => "Copy image address",
            Self::Edit(FrameEdit::Copy) => "Copy",
            Self::Edit(FrameEdit::Cut) => "Cut",
            Self::Edit(FrameEdit::Paste) => "Paste",
            Self::Edit(FrameEdit::SelectAll) => "Select all",
            Self::Navigate(Navigation::Back) => "Back",
            Self::Navigate(Navigation::Forward) => "Forward",
            Self::Navigate(Navigation::Reload) => "Reload",
            Self::Navigate(_) => unreachable!(),
            Self::Inspect => "Inspect element",
        }
    }
}

fn items(context: &MenuContext) -> Vec<(Item, bool)> {
    let mut items = Vec::new();
    if !context.link_url.is_empty() {
        let navigable = pinnable_address(&context.link_url).is_some();
        items.extend([
            (Item::Link(Destination::Tab), navigable),
            (Item::Link(Destination::Peek), navigable),
            (Item::Link(Destination::Incognito), navigable),
            (Item::CopyLink, true),
        ]);
    }
    if !context.src_url.is_empty() {
        items.extend([
            (Item::CopyImage, true),
            (Item::Image, pinnable_address(&context.src_url).is_some()),
        ]);
    }
    let selected = !context.selection_text.is_empty();
    if context.is_editable {
        items.push((Item::Edit(FrameEdit::Cut), selected && context.can_cut));
    }
    if selected || context.is_editable {
        items.push((Item::Edit(FrameEdit::Copy), selected && context.can_copy));
        if context.is_editable {
            items.push((Item::Edit(FrameEdit::Paste), true));
        }
        items.push((Item::Edit(FrameEdit::SelectAll), true));
    }
    items.extend([
        (Item::Navigate(Navigation::Back), context.can_go_back),
        (Item::Navigate(Navigation::Forward), context.can_go_forward),
        (Item::Navigate(Navigation::Reload), true),
        (Item::Inspect, true),
    ]);
    items
}

impl Controller {
    fn browser_window(&self, key: &BrowserKey) -> Option<&WindowId> {
        if self.engine.key(&key.tab).as_ref() != Some(key) {
            return None;
        }
        match self.engine.owner(&key.tab)? {
            Owner::Tab(id) | Owner::Peek(id) | Owner::Flechinha(id) | Owner::Popup(id) => Some(id),
        }
    }

    pub(crate) fn poll_page_overlays(&mut self, cx: &mut App) {
        while let Some(event) = self.engine.next_context_menu_event() {
            let id = match &event {
                ContextMenuEvent::Open(request) => &request.id,
                ContextMenuEvent::Dismissed(id) => id,
            };
            if let Some(handle) = self
                .browser_window(&id.key)
                .and_then(|id| self.windows.get(id))
                .copied()
            {
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |shell, window, cx| {
                        shell.receive_context_menu(event, window, cx)
                    });
                });
            } else {
                self.engine.dismiss_context_menu(id);
            }
        }
        while let Some(event) = self.engine.next_find_event() {
            if let Some(handle) = self
                .browser_window(&event.key)
                .and_then(|id| self.windows.get(id))
                .copied()
            {
                cx.defer(move |cx| {
                    let _ = handle.update(cx, |shell, _, cx| shell.receive_find(event, cx));
                });
            }
        }
    }

    pub(crate) fn open_context_url(
        &mut self,
        owner: Owner,
        source: &TabId,
        url: &str,
        target: Destination,
    ) -> Result<()> {
        let url = pinnable_address(url).context("This address cannot be opened in a new page")?;
        if target == Destination::Incognito {
            let window = self.model.open_incognito_window();
            self.model.open(&window, url, true)?;
            self.front = Some(window);
            return Ok(());
        }

        let partition = self
            .model
            .partition(source)
            .context("The source page has closed")?;
        let window = match place(&self.model, &owner, source, Disposition::BackgroundTab) {
            Place::Tab {
                window,
                after,
                activate,
            } => {
                if target == Destination::Tab {
                    self.model
                        .open_page(tab_id(), &window, url, after.as_ref(), activate)?;
                    if matches!(owner, Owner::Flechinha(_) | Owner::Popup(_)) {
                        self.front = Some(window);
                    }
                    return Ok(());
                }
                window
            }
            Place::Nowhere => {
                if target == Destination::Tab {
                    self.front = Some(self.model.open_window_page(tab_id(), url, &partition)?);
                    return Ok(());
                }
                if partition.is_incognito() {
                    self.model.open_incognito_window()
                } else {
                    self.model.open_regular_window()?
                }
            }
            _ => unreachable!("background links open in a tab or have no target window"),
        };
        if let Some(old) = self.model.peek(window.clone(), url, Some(source))? {
            self.engine.close(&old);
        }
        self.front = Some(window);
        Ok(())
    }
}

impl Shell {
    fn receive_context_menu(
        &mut self,
        event: ContextMenuEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ContextMenuEvent::Open(request) => {
                if self.active_browser().as_ref() == Some(&request.id.key)
                    && !self.overlay.is_open()
                    && self.state.borrow().engine.context_menu_is_open(&request.id)
                {
                    self.show_overlay(Overlay::ContextMenu(request), window, cx);
                } else {
                    self.state.borrow().engine.dismiss_context_menu(&request.id);
                }
            }
            ContextMenuEvent::Dismissed(id) => {
                if matches!(&self.overlay, Overlay::ContextMenu(request) if request.id == id) {
                    self.dismiss_overlay(window, cx);
                }
            }
        }
    }

    /// The open page menu's enabled row labelled `label`.
    pub(crate) fn context_menu_row(&self, label: &str) -> Result<Item> {
        let Overlay::ContextMenu(request) = &self.overlay else {
            anyhow::bail!("no page menu is open");
        };
        items(&request.context)
            .into_iter()
            .find_map(|(item, enabled)| (enabled && item.label() == label).then_some(item))
            .with_context(|| format!("the page menu has no enabled {label} row"))
    }

    pub(crate) fn use_context_menu(
        &mut self,
        item: Item,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shell_clipboard = self.shell_clipboard(window);
        let Overlay::ContextMenu(request) = &self.overlay else {
            return;
        };
        let id = request.id.clone();
        let position = request.position;
        if self.active_browser().as_ref() != Some(&id.key)
            || !self.state.borrow().engine.context_menu_is_open(&id)
        {
            self.dismiss_overlay(window, cx);
            return;
        }
        let action = match item {
            Item::Link(destination) => Some(Action::OpenContext(
                id.key.tab.clone(),
                request.context.link_url.clone(),
                destination,
            )),
            Item::Image => Some(Action::OpenContext(
                id.key.tab.clone(),
                request.context.src_url.clone(),
                Destination::Tab,
            )),
            Item::Navigate(nav) => Some(Action::Browser(nav)),
            Item::CopyLink | Item::CopyImage => {
                let url = if item == Item::CopyLink {
                    &request.context.link_url
                } else {
                    &request.context.src_url
                };
                cx.write_to_clipboard(ClipboardItem::new_string(url.clone()));
                None
            }
            Item::Edit(action @ (FrameEdit::Paste | FrameEdit::Copy | FrameEdit::Cut))
                if shell_clipboard && self.state.borrow().holding(&id.key) =>
            {
                // A copy or cut in this page is still reading, so the edit waits its turn.
                let context = &request.context;
                let mut state = self.state.borrow_mut();
                let focused = state.engine.context_menu_frame_focused(&id);
                let held = match action {
                    FrameEdit::Paste => focused.then_some(Held::Paste(None)),
                    FrameEdit::Cut => {
                        (focused && context.can_cut).then_some(Held::Read(ClipboardEdit::Cut, None))
                    }
                    _ => (context.can_copy && !context.selection_text.is_empty())
                        .then(|| Held::Write(context.selection_text.clone())),
                };
                if let Some(held) = held {
                    state.hold(&id.key, held);
                }
                state.engine.dismiss_context_menu(&id);
                None
            }
            Item::Edit(FrameEdit::Paste)
                if shell_clipboard && let Some(text) = crate::clipboard::text(cx) =>
            {
                let focused = self.state.borrow().engine.context_menu_frame_focused(&id);
                // Dismissing gives the page its focus back before the text arrives.
                self.dismiss_overlay(window, cx);
                if focused {
                    self.state.borrow().engine.paste_text(&id.key.tab, &text);
                }
                return;
            }
            Item::Edit(action) => {
                let context = &request.context;
                let mut state = self.state.borrow_mut();
                // Chromium's Copy and Cut would take the clipboard from the shell. See
                // `Engine::copy_selection`.
                match action {
                    FrameEdit::Copy
                        if shell_clipboard
                            && context.can_copy
                            && !context.selection_text.is_empty() =>
                    {
                        cx.write_to_clipboard(ClipboardItem::new_string(
                            context.selection_text.clone(),
                        ));
                        state.engine.dismiss_context_menu(&id);
                    }
                    // The selection may have changed since the menu opened, so the cut reads
                    // it again and deletes what it read. The read goes to the focused frame.
                    FrameEdit::Cut if shell_clipboard && context.can_cut => {
                        if state.engine.context_menu_frame_focused(&id) {
                            state.read_selection(&id.key, ClipboardEdit::Cut, None);
                        }
                        state.engine.dismiss_context_menu(&id);
                    }
                    _ => state.engine.context_menu_edit(&id, action),
                }
                None
            }
            Item::Inspect => None,
        };
        self.dismiss_overlay(window, cx);
        if let Some(action) = action {
            dispatch(&self.state, &self.id, action, cx);
        }
        if item == Item::Inspect {
            self.state
                .borrow()
                .engine
                .show_dev_tools(&id.key.tab, Some(position));
        }
    }

    pub(crate) fn context_menu_panel(
        &self,
        menu: &ContextMenuRequest,
        window: &Window,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        const WIDTH: Pixels = px(218.);
        const ROW: Pixels = px(28.);
        const PADDING: Pixels = px(4.);
        const BORDER: Pixels = px(1.);
        let rows = items(&menu.context);
        let viewport = window.viewport_size();
        let width = WIDTH.min(viewport.width);
        let height = (ROW * rows.len() as f32 + (PADDING + BORDER) * 2.).min(viewport.height);
        let left = menu.anchor.x.clamp(px(0.), viewport.width - width);
        let top = menu.anchor.y.clamp(px(0.), viewport.height - height);
        deferred(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .child(div().absolute().inset_0().on_any_mouse_down(cx.listener(
                    |this, _: &MouseDownEvent, window, cx| {
                        this.dismiss_overlay(window, cx);
                        cx.stop_propagation();
                    },
                )))
                .child(
                    div()
                        .id("page-context-menu")
                        .absolute()
                        .left(left)
                        .top(top)
                        .w(width)
                        .h(height)
                        .occlude()
                        .overflow_y_scroll()
                        .p(PADDING)
                        .rounded_lg()
                        .border_1()
                        .border_color(palette().border)
                        .bg(palette().panel)
                        .shadow_lg()
                        .text_sm()
                        .children(rows.into_iter().map(|(item, enabled)| {
                            let label = item.label();
                            if enabled {
                                button(label, label)
                                    .h(ROW)
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, window, cx| {
                                            this.use_context_menu(item, window, cx);
                                        },
                                    ))
                                    .into_any_element()
                            } else {
                                div()
                                    .h(ROW)
                                    .px_2()
                                    .py_1()
                                    .text_color(palette().disabled)
                                    .child(label)
                                    .into_any_element()
                            }
                        })),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_rows_follow_page_context_and_navigation_history() {
        use Destination::*;
        use FrameEdit::*;
        use Item::*;
        use Navigation::*;
        let default = MenuContext::default();
        let baseline = [
            (Navigate(Back), false),
            (Navigate(Forward), false),
            (Navigate(Reload), true),
            (Inspect, true),
        ];
        assert_eq!(items(&default), baseline);
        for (context, expected) in [
            (
                MenuContext {
                    link_url: "https://example.org/".into(),
                    ..MenuContext::default()
                },
                vec![
                    (Link(Tab), true),
                    (Link(Peek), true),
                    (Link(Incognito), true),
                    (CopyLink, true),
                ],
            ),
            (
                MenuContext {
                    selection_text: "selected".into(),
                    can_copy: true,
                    ..MenuContext::default()
                },
                vec![(Edit(Copy), true), (Edit(SelectAll), true)],
            ),
            (
                MenuContext {
                    is_editable: true,
                    selection_text: "selected".into(),
                    can_copy: true,
                    can_cut: true,
                    ..MenuContext::default()
                },
                vec![
                    (Edit(Cut), true),
                    (Edit(Copy), true),
                    (Edit(Paste), true),
                    (Edit(SelectAll), true),
                ],
            ),
            // Chromium refuses to copy or cut a password field's selection.
            (
                MenuContext {
                    is_editable: true,
                    selection_text: "secret".into(),
                    ..MenuContext::default()
                },
                vec![
                    (Edit(Cut), false),
                    (Edit(Copy), false),
                    (Edit(Paste), true),
                    (Edit(SelectAll), true),
                ],
            ),
            (
                MenuContext {
                    is_editable: true,
                    ..MenuContext::default()
                },
                vec![
                    (Edit(Cut), false),
                    (Edit(Copy), false),
                    (Edit(Paste), true),
                    (Edit(SelectAll), true),
                ],
            ),
            (
                MenuContext {
                    src_url: "https://example.org/image.png".into(),
                    ..MenuContext::default()
                },
                vec![(CopyImage, true), (Image, true)],
            ),
        ] {
            assert_eq!(items(&context), [expected.as_slice(), &baseline].concat());
        }
        assert_eq!(
            items(&MenuContext {
                can_go_back: true,
                can_go_forward: true,
                ..default
            })[..2],
            [(Navigate(Back), true), (Navigate(Forward), true)]
        );
    }

    #[test]
    fn link_actions_from_incognito_keep_the_existing_private_partition() {
        let profile = tempfile::tempdir().unwrap();
        let mut controller = Controller::open_test_profile(profile.path()).unwrap();
        let window = controller.model.open_incognito_window();
        let source = controller
            .model
            .open(&window, "https://example.org/".into(), true)
            .unwrap();
        let partition = controller.model.partition(&source).unwrap();
        assert!(partition.is_incognito());
        let owner = Owner::Tab(window.clone());
        controller
            .open_context_url(
                owner.clone(),
                &source,
                "https://example.org/peek",
                Destination::Peek,
            )
            .unwrap();
        assert_eq!(controller.model.peeks[&window].partition, partition);
        controller
            .open_context_url(
                owner.clone(),
                &source,
                "https://example.org/tab",
                Destination::Tab,
            )
            .unwrap();
        let state = controller.model.state(&window).unwrap();
        let tab = state
            .tab_ids(&window, &state.window(&window).unwrap().active_space_id)
            .unwrap()
            .into_iter()
            .find(|id| state.tab(id).unwrap().url.ends_with("/tab"))
            .unwrap();
        assert_eq!(controller.model.partition(tab), Some(partition.clone()));
        controller
            .open_context_url(
                owner,
                &source,
                "https://example.org/private",
                Destination::Incognito,
            )
            .unwrap();
        let private = controller.front.as_ref().unwrap();
        assert_ne!(private, &window);
        let tab = controller.model.active(private).unwrap();
        assert_eq!(
            controller.model.tab(tab).unwrap().url,
            "https://example.org/private"
        );
        assert_eq!(controller.model.partition(tab), Some(partition));
    }
}
