use anyhow::{Context as _, Result};
use flecha_cef::{
    BrowserKey, ClipboardEdit, ContextMenuEvent, ContextMenuRequest, Disposition, FrameEdit,
    MenuContext, MenuFrame, Owner,
};
use flecha_core::{SearchEngine, TabId, WindowId, suggestions::pinnable_address};
use gpui::{
    App, ClickEvent, ClipboardItem, Context, InteractiveElement as _, IntoElement, MouseDownEvent,
    ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _, Styled as _, Window,
    deferred, div, px,
};

use crate::clipboard::Held;
use crate::model::tab_id;
use crate::popup::{Place, place};
use crate::shortcuts::Navigation;
use crate::theme::palette;
use crate::tokens;
use crate::view::{Overlay, Shell, menu_item};
use crate::{Action, Controller, dispatch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Destination {
    /// A tab behind the source, as a link's "Open link in new tab" opens.
    Tab,
    /// A tab in front, as a search for the selection opens.
    FrontTab,
    Peek,
    Incognito,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Item {
    Link(Destination),
    SplitLink,
    SaveLink,
    CopyLink,
    Image,
    SaveImage,
    CopyImage,
    CopyImageAddress,
    Edit(FrameEdit),
    Search,
    Navigate(Navigation),
    SavePage,
    Print,
    ViewSource,
    Inspect,
}

/// The longest selection a Search row quotes (`kMaxSelectionTextLength` in Chromium's
/// render_view_context_menu.cc).
const SEARCH_LABEL_SELECTION: usize = 50;

/// What the menu's rows depend on beyond the page: the search engine Search names, and whether
/// the page is a Flecha window's tab, which alone can open a link in a split.
#[derive(Clone, Copy)]
pub(crate) struct MenuPlace {
    engine: SearchEngine,
    splits: bool,
}

impl MenuPlace {
    fn of(state: &Controller, tab: &TabId) -> Self {
        Self {
            engine: state.model.core.settings.search_engine,
            // The responsive view's phone mirror is half of a split already, which core
            // refuses to split again.
            splits: matches!(state.engine.owner(tab), Some(Owner::Tab(_)))
                && state
                    .model
                    .tab(tab)
                    .is_some_and(|tab| tab.device != Some(flecha_core::Device::Mobile)),
        }
    }
}

impl Item {
    /// The row's label, Chromium's own (chrome/app/generated_resources.grd).
    fn label(self, context: &MenuContext, place: MenuPlace) -> SharedString {
        SharedString::new_static(match self {
            Self::Link(Destination::Tab | Destination::FrontTab) => "Open link in new tab",
            Self::Link(Destination::Peek) => "Peek link",
            Self::Link(Destination::Incognito) => "Open link in Incognito",
            Self::SplitLink => "Open link in split view",
            Self::SaveLink => "Save link as…",
            Self::CopyLink => "Copy link address",
            Self::Image => "Open image in new tab",
            Self::SaveImage => "Save image as…",
            Self::CopyImage => "Copy image",
            Self::CopyImageAddress => "Copy image address",
            Self::Edit(FrameEdit::Copy) => "Copy",
            Self::Edit(FrameEdit::Cut) => "Cut",
            Self::Edit(FrameEdit::Paste) => "Paste",
            Self::Edit(FrameEdit::SelectAll) => "Select all",
            Self::Search => {
                return format!(
                    "Search {} for “{}”",
                    place.engine.name(),
                    search_label_text(&context.selection_text)
                )
                .into();
            }
            Self::Navigate(Navigation::Back) => "Back",
            Self::Navigate(Navigation::Forward) => "Forward",
            Self::Navigate(Navigation::Reload) => "Reload",
            Self::Navigate(_) => unreachable!(),
            Self::SavePage => "Save as…",
            Self::Print => "Print…",
            Self::ViewSource => "View page source",
            Self::Inspect => "Inspect element",
        })
    }
}

/// The selection as a Search row quotes it: on one line, and cut short with an ellipsis.
fn search_label_text(selection: &str) -> String {
    let line = selection.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = line.chars();
    let quoted: String = chars.by_ref().take(SEARCH_LABEL_SELECTION).collect();
    if chars.next().is_some() {
        format!("{quoted}…")
    } else {
        quoted
    }
}

/// Whether Flecha can save or show the source of a page at `url`.
fn saveable(url: &str) -> bool {
    ["https:", "http:", "file:"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
}

fn items(context: &MenuContext, place: MenuPlace) -> Vec<(Item, bool)> {
    let mut items = Vec::new();
    if !context.link_url.is_empty() {
        let navigable = pinnable_address(&context.link_url).is_some();
        items.extend([
            (Item::Link(Destination::Tab), navigable),
            (Item::Link(Destination::Peek), navigable),
            (Item::Link(Destination::Incognito), navigable),
        ]);
        if place.splits {
            items.push((Item::SplitLink, navigable));
        }
        items.extend([
            (Item::SaveLink, saveable(&context.link_url)),
            (Item::CopyLink, true),
        ]);
    }
    if !context.src_url.is_empty() {
        items.push((Item::Image, pinnable_address(&context.src_url).is_some()));
        if context.is_image {
            items.extend([
                (Item::SaveImage, saveable(&context.src_url)),
                (Item::CopyImage, true),
            ]);
        }
        items.push((Item::CopyImageAddress, true));
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
    // Chromium offers no search for a password field's selection, which it won't copy either.
    if context.can_copy && !context.selection_text.trim().is_empty() {
        items.push((Item::Search, true));
    }
    let page = saveable(&context.page_url);
    items.extend([
        (Item::Navigate(Navigation::Back), context.can_go_back),
        (Item::Navigate(Navigation::Forward), context.can_go_forward),
        (Item::Navigate(Navigation::Reload), true),
        (Item::SavePage, page),
        (Item::Print, true),
        (Item::ViewSource, page),
        (Item::Inspect, true),
    ]);
    items
}

impl Controller {
    pub(crate) fn browser_window(&self, key: &BrowserKey) -> Option<&WindowId> {
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
        // Placed as a background link, so a pinned tab's search opens a tab rather than Peek.
        let in_tab = matches!(target, Destination::Tab | Destination::FrontTab);
        let window = match place(&self.model, &owner, source, Disposition::BackgroundTab) {
            Place::Tab {
                window,
                after,
                activate,
            } => {
                if in_tab {
                    let activate = activate || target == Destination::FrontTab;
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
                if in_tab {
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
        let place = MenuPlace::of(&self.state.borrow(), &request.id.key.tab);
        items(&request.context, place)
            .into_iter()
            .find_map(|(item, enabled)| {
                (enabled && item.label(&request.context, place) == label).then_some(item)
            })
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
            Item::SplitLink => Some(Action::SplitNew(request.context.link_url.clone())),
            Item::Search => {
                let engine = self.state.borrow().model.core.settings.search_engine;
                Some(Action::OpenContext(
                    id.key.tab.clone(),
                    engine.search_url(request.context.selection_text.trim()),
                    Destination::FrontTab,
                ))
            }
            Item::Navigate(nav) => Some(Action::Browser(nav)),
            Item::Print => Some(Action::Print),
            Item::ViewSource => Some(Action::ViewSource),
            Item::SaveLink | Item::SaveImage | Item::SavePage | Item::CopyImage => {
                let context = &request.context;
                let mut state = self.state.borrow_mut();
                let tab = &id.key.tab;
                let done = match item {
                    Item::SaveLink => state.engine.save_as(tab, &context.link_url),
                    Item::SaveImage => state.engine.save_as(tab, &context.src_url),
                    // Saved from the network again, not from the page as it stands.
                    Item::SavePage => state.engine.save_as(tab, &context.page_url),
                    _ => state.engine.copy_image(tab, &context.src_url),
                };
                if let Err(error) = done {
                    let session = state
                        .model
                        .partition(tab)
                        .and_then(|partition| partition.incognito_session());
                    state.status.set(session, format!("{error:#}"));
                }
                None
            }
            Item::CopyLink | Item::CopyImageAddress => {
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
                let frame = state
                    .engine
                    .context_menu_frame(&id)
                    .filter(MenuFrame::is_focused);
                let held = match action {
                    FrameEdit::Paste => frame.map(Held::MenuPaste),
                    FrameEdit::Cut => context
                        .can_cut
                        .then_some(frame)
                        .flatten()
                        .map(Held::MenuCut),
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
        // `.shell-menu` and `.shell-menu-item` (chromium/shell/shell.css:72-113): one line of
        // body text with 4px above and below, in a 5px padded box ringed by `--shadow-pop`.
        const ROW: Pixels =
            px(tokens::MENU_ITEM_PAD_Y * 2. + tokens::TEXT_MD * tokens::LINE_HEIGHT);
        const PADDING: Pixels = px(tokens::MENU_PAD);
        const BORDER: Pixels = px(0.);
        let place = MenuPlace::of(&self.state.borrow(), &menu.id.key.tab);
        let rows = items(&menu.context, place);
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
                        .rounded(px(tokens::RADIUS_MD))
                        .bg(palette().surface)
                        .text_color(palette().text)
                        .shadow(palette().shadow_pop())
                        .children(rows.into_iter().map(|(item, enabled)| {
                            let label = item.label(&menu.context, place);
                            if enabled {
                                menu_item(label.clone(), label)
                                    .truncate()
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
                                    .px(px(tokens::MENU_ITEM_PAD_X))
                                    .py(px(tokens::MENU_ITEM_PAD_Y))
                                    .opacity(tokens::MENU_DISABLED_OPACITY)
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
        let tab = MenuPlace {
            engine: SearchEngine::Google,
            splits: true,
        };
        let default = MenuContext::default();
        let baseline = [
            (Navigate(Back), false),
            (Navigate(Forward), false),
            (Navigate(Reload), true),
            (SavePage, false),
            (Print, true),
            (ViewSource, false),
            (Inspect, true),
        ];
        assert_eq!(items(&default, tab), baseline);
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
                    (SplitLink, true),
                    (SaveLink, true),
                    (CopyLink, true),
                ],
            ),
            (
                MenuContext {
                    link_url: "javascript:void(0)".into(),
                    ..MenuContext::default()
                },
                vec![
                    (Link(Tab), false),
                    (Link(Peek), false),
                    (Link(Incognito), false),
                    (SplitLink, false),
                    (SaveLink, false),
                    (CopyLink, true),
                ],
            ),
            (
                MenuContext {
                    selection_text: "selected".into(),
                    can_copy: true,
                    ..MenuContext::default()
                },
                vec![(Edit(Copy), true), (Edit(SelectAll), true), (Search, true)],
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
                    (Search, true),
                ],
            ),
            // Chromium refuses to copy or cut a password field's selection, or search for it.
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
            // A video's or audio's source has no image rows.
            (
                MenuContext {
                    src_url: "https://example.org/clip.webm".into(),
                    ..MenuContext::default()
                },
                vec![(Image, true), (CopyImageAddress, true)],
            ),
            (
                MenuContext {
                    src_url: "https://example.org/image.png".into(),
                    is_image: true,
                    ..MenuContext::default()
                },
                vec![
                    (Image, true),
                    (SaveImage, true),
                    (CopyImage, true),
                    (CopyImageAddress, true),
                ],
            ),
        ] {
            assert_eq!(
                items(&context, tab),
                [expected.as_slice(), &baseline].concat()
            );
        }
        assert_eq!(
            items(
                &MenuContext {
                    can_go_back: true,
                    can_go_forward: true,
                    ..MenuContext::default()
                },
                tab
            )[..2],
            [(Navigate(Back), true), (Navigate(Forward), true)]
        );
        // A web page can be saved and its source shown.
        let page = MenuContext {
            page_url: "https://example.org/".into(),
            ..MenuContext::default()
        };
        assert!(items(&page, tab).contains(&(SavePage, true)));
        assert!(items(&page, tab).contains(&(ViewSource, true)));
        // Only a Flecha window's tab opens a link in a split.
        let link = MenuContext {
            link_url: "https://example.org/".into(),
            ..MenuContext::default()
        };
        let elsewhere = MenuPlace {
            splits: false,
            ..tab
        };
        assert!(
            !items(&link, elsewhere)
                .iter()
                .any(|(item, _)| *item == SplitLink)
        );
    }

    #[test]
    fn the_search_row_names_the_engine_and_quotes_the_selection_on_one_line() {
        let place = MenuPlace {
            engine: SearchEngine::Google,
            splits: true,
        };
        let label = |selection: &str| {
            Item::Search.label(
                &MenuContext {
                    selection_text: selection.into(),
                    ..MenuContext::default()
                },
                place,
            )
        };
        assert_eq!(
            label("  rust\n  borrow checker "),
            format!(
                "Search {} for “rust borrow checker”",
                SearchEngine::Google.name()
            )
        );
        let long = "a".repeat(SEARCH_LABEL_SELECTION + 10);
        assert_eq!(
            label(&long),
            format!(
                "Search {} for “{}…”",
                SearchEngine::Google.name(),
                "a".repeat(SEARCH_LABEL_SELECTION)
            )
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
