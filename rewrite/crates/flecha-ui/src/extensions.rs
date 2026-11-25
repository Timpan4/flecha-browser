use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context as _, Result, ensure};
use flecha_cef::{
    Engine, Owner, Partition,
    extensions::{Extension, ExtensionSettings},
};
use flecha_core::{TabId, WindowId};
use gpui::{
    Anchor, AnyElement, Bounds, ClickEvent, Context, InteractiveElement as _, IntoElement as _,
    MouseButton, MouseDownEvent, ParentElement as _, Pixels, RenderImage,
    StatefulInteractiveElement as _, Styled as _, Window, anchored, canvas, deferred, div,
    prelude::FluentBuilder as _, px,
};

use crate::settings_view::SettingsTab;
use crate::theme::palette;
use crate::tokens;
use crate::view::{Overlay, Shell, icon_button, overlay_panel, text_button};
use crate::{Controller, model};

pub(crate) struct Extensions {
    pub settings: ExtensionSettings,
    pub managed: BTreeSet<String>,
    pub folders: BTreeSet<PathBuf>,
    pub pins: BTreeSet<String>,
    startup: BTreeSet<PathBuf>,
    /// Why the extension settings files could not be read.
    pub problem: Option<String>,
    icons: BTreeMap<PathBuf, Option<Arc<RenderImage>>>,
    popups: BTreeMap<WindowId, ExtensionPopup>,
}

pub(crate) struct ExtensionPopup {
    pub tab: TabId,
    pub partition: Partition,
    pub url: String,
    id: String,
    name: String,
    source: Option<TabId>,
}

impl Extensions {
    /// A settings file Flecha can't read leaves its list empty and shows the error in Settings,
    /// so the browser still opens.
    pub fn new(profile: &Path) -> Self {
        let settings = ExtensionSettings::new(profile);
        let mut extensions = Self {
            startup: settings.startup_folders().unwrap_or_default(),
            settings,
            managed: BTreeSet::new(),
            pins: BTreeSet::new(),
            folders: BTreeSet::new(),
            problem: None,
            icons: BTreeMap::new(),
            popups: BTreeMap::new(),
        };
        let _ = extensions.reload_settings();
        extensions
    }

    /// Replaces the lists only when every file reads, and records the failure otherwise.
    pub fn reload_settings(&mut self) -> Result<()> {
        let read = (|| {
            Ok::<_, anyhow::Error>((
                self.settings.managed_ids()?,
                self.settings.unpacked()?,
                self.settings.pinned()?,
            ))
        })();
        match read {
            Ok((managed, folders, pins)) => {
                (self.managed, self.folders, self.pins) = (managed, folders, pins);
                self.problem = None;
                Ok(())
            }
            Err(error) => {
                self.problem = Some(format!("{error:#}"));
                Err(error)
            }
        }
    }

    pub fn starting_folders(&self) -> &BTreeSet<PathBuf> {
        &self.startup
    }

    pub fn refresh_icons(&mut self, engine: &Engine) {
        let icons: BTreeSet<_> = engine
            .context_partitions()
            .flat_map(|partition| engine.installed_extensions(partition))
            .filter_map(|extension| extension.icon.as_ref())
            .cloned()
            .collect();
        self.icons.retain(|path, _| icons.contains(path));
        for path in icons {
            self.icons.entry(path.clone()).or_insert_with(|| {
                let bitmap = image::ImageReader::open(path)
                    .ok()?
                    .with_guessed_format()
                    .ok()?
                    .decode()
                    .ok()?
                    .into_rgba8();
                let (width, height) = bitmap.dimensions();
                let mut data = bitmap.into_raw();
                for pixel in data.as_chunks_mut::<4>().0 {
                    pixel.swap(0, 2);
                }
                let image = image::RgbaImage::from_raw(width, height, data)?;
                Some(Arc::new(RenderImage::new(vec![image::Frame::new(image)])))
            });
        }
    }

    pub fn icon(&self, extension: &Extension) -> Option<Arc<RenderImage>> {
        self.icons.get(extension.icon.as_ref()?)?.clone()
    }

    pub fn popup(&self, window: &WindowId) -> Option<&ExtensionPopup> {
        self.popups.get(window)
    }

    pub fn popup_by_tab(&self, tab: &TabId) -> Option<&ExtensionPopup> {
        self.popups.values().find(|popup| popup.tab == *tab)
    }

    pub fn has_popup(&self, tab: &TabId) -> bool {
        self.popup_by_tab(tab).is_some()
    }

    pub fn forget_popup(&mut self, tab: &TabId) {
        self.popups.retain(|_, popup| popup.tab != *tab);
    }
}

impl Controller {
    pub fn extension_partition(&self, id: &WindowId) -> Option<Partition> {
        let window = self.model.core.window(id)?;
        let partition = self
            .model
            .keyboard_pane(id)
            .and_then(|(_, tab)| self.model.partition(tab))
            .unwrap_or_else(|| self.model.window_partition(id, &window.active_space_id));
        matches!(partition, Partition::Shared | Partition::Isolated(_)).then_some(partition)
    }

    pub fn close_extension_popup(&mut self, id: &WindowId) {
        if let Some(popup) = self.extensions.popups.remove(id) {
            self.engine.close(&popup.tab);
        }
    }

    pub fn extension_popup_current(&self, window: &WindowId) -> bool {
        self.extensions.popup(window).is_some_and(|popup| {
            self.model.keyboard_pane(window).map(|(_, tab)| tab) == popup.source.as_ref()
                && self.extension_partition(window).as_ref() == Some(&popup.partition)
                && self
                    .engine
                    .installed_extensions(&popup.partition)
                    .iter()
                    .any(|extension| extension.id == popup.id && extension.enabled)
        })
    }

    fn start_extension_popup(&mut self, window: &WindowId, id: &str) -> Result<()> {
        let partition = self
            .extension_partition(window)
            .context("Extensions are disabled in this tab")?;
        let extension = self
            .engine
            .installed_extensions(&partition)
            .iter()
            .find(|extension| extension.id == id)
            .context("Extension is unavailable in this Space")?;
        ensure!(extension.enabled, "This extension is disabled");
        let url = extension
            .popup
            .clone()
            .context("This action has no popup. Stock CEF cannot run its click handler.")?;
        let popup = ExtensionPopup {
            tab: model::tab_id(),
            partition,
            url,
            id: id.to_owned(),
            name: extension.name.clone(),
            source: self.model.keyboard_pane(window).map(|(_, tab)| tab.clone()),
        };
        self.close_extension_popup(window);
        self.extensions.popups.insert(window.clone(), popup);
        Ok(())
    }
}

impl Shell {
    fn run_extension(
        &mut self,
        id: &str,
        anchor: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dismiss_overlay(window, cx);
        let result = self.state.borrow_mut().start_extension_popup(&self.id, id);
        match result {
            Ok(()) => {
                self.show_overlay(Overlay::ExtensionPopup(anchor), window, cx);
                self.page_focus.focus(window, cx);
                self.sync_page_focus(window, cx);
            }
            Err(error) => {
                self.show_overlay(
                    Overlay::Extensions(anchor, Some(error.to_string())),
                    window,
                    cx,
                );
            }
        }
    }

    pub(crate) fn extension_actions(
        &self,
        state: &Controller,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let partition = state.extension_partition(&self.id)?;
        let extensions = state.engine.installed_extensions(&partition);
        if extensions.is_empty() {
            return None;
        }
        let mut row = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(tokens::TOOLBAR_GAP));
        let bounds = Rc::new(Cell::new(Bounds::default()));
        row = row.child(
            icon_button("extensions-menu", "puzzle", "Extensions")
                .relative()
                .child(self.track_extension_control("menu".into(), bounds.clone()))
                .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                    this.show_overlay(Overlay::Extensions(bounds.get(), None), window, cx);
                })),
        );
        for extension in extensions.iter().filter(|extension| {
            extension.has_action && state.extensions.pins.contains(&extension.id)
        }) {
            let bounds = Rc::new(Cell::new(Bounds::default()));
            let id = extension.id.clone();
            let menu_bounds = bounds.clone();
            row = row.child(
                div()
                    .id(format!("extension-{id}"))
                    .relative()
                    .size(px(tokens::ICON_BUTTON))
                    .rounded(px(tokens::RADIUS_SM))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .aria_label(extension.name.clone())
                    .hover(|button| button.bg(palette().fill_hover))
                    .when(!extension.enabled, |button| {
                        button.opacity(tokens::MENU_DISABLED_OPACITY)
                    })
                    .child(self.extension_icon(state, extension))
                    .child(self.track_extension_control(format!("action-{id}"), bounds.clone()))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.run_extension(&id, bounds.get(), window, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                            this.show_overlay(
                                Overlay::Extensions(menu_bounds.get(), None),
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    ),
            );
        }
        Some(row.into_any_element())
    }

    pub(crate) fn extension_icon(&self, state: &Controller, extension: &Extension) -> AnyElement {
        self.site_icon(
            state.extensions.icon(extension),
            &extension.name,
            (
                tokens::ICON_BUTTON,
                tokens::RADIUS_SM,
                tokens::FAVICON_LETTER,
            ),
        )
    }

    pub(crate) fn extensions_menu(
        &self,
        anchor: Bounds<Pixels>,
        message: Option<&str>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let state = self.state.borrow();
        let mut panel = overlay_panel().w(px(tokens::SETTINGS_NAV));
        for extension in self.extension_entries(&state) {
            let id = extension.id.clone();
            let pin = id.clone();
            let pinned = state.extensions.pins.contains(&pin);
            let action = div()
                .flex_1()
                .min_w_0()
                .truncate()
                .child(extension.name.clone());
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(tokens::SPACE_2))
                    .child(self.extension_icon(&state, extension))
                    .child(if extension.has_action {
                        action
                            .id(format!("run-{id}"))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.run_extension(&id, anchor, window, cx);
                            }))
                            .into_any_element()
                    } else {
                        action.into_any_element()
                    })
                    .when(extension.has_action, |row| {
                        row.child(
                            text_button(format!("pin-{pin}"), if pinned { "Unpin" } else { "Pin" })
                                .relative()
                                .child(
                                    self.track_extension_control(
                                        format!("pin-{pin}"),
                                        Rc::default(),
                                    ),
                                )
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    let mut state = this.state.borrow_mut();
                                    let settings = &state.extensions.settings;
                                    match settings.toggle_pin(&pin).and_then(|()| settings.pinned())
                                    {
                                        Ok(pins) => state.extensions.pins = pins,
                                        Err(error) => state.status.set(None, format!("{error:#}")),
                                    }
                                    cx.refresh_windows();
                                })),
                        )
                    }),
            );
        }
        panel = panel
            .when_some(message, |panel, text| {
                panel.child(div().text_size(px(tokens::TEXT_SM)).child(text.to_owned()))
            })
            .child(
                text_button("manage-extensions", "Manage Extensions").on_click(cx.listener(
                    |this, _: &ClickEvent, window, cx| {
                        this.toggle_settings(SettingsTab::Extensions, window, cx);
                    },
                )),
            );
        extension_overlay(anchor, panel, cx)
    }

    pub(crate) fn extension_entries<'a>(&self, state: &'a Controller) -> &'a [Extension] {
        state
            .extension_partition(&self.id)
            .map_or(&[], |partition| {
                state.engine.installed_extensions(&partition)
            })
    }

    pub(crate) fn track_extension_control(
        &self,
        id: String,
        bounds: Rc<Cell<Bounds<Pixels>>>,
    ) -> impl gpui::IntoElement {
        let verify = self.state.borrow().verify;
        let owner = self.id.clone();
        canvas(
            |_, _, _| (),
            move |area, (), window, _| {
                bounds.set(area);
                if verify {
                    crate::verify::extension_control(
                        &owner,
                        &id,
                        window.bounds().origin + area.center(),
                    );
                }
            },
        )
        .absolute()
        .inset_0()
    }

    pub(crate) fn extension_popover(
        &self,
        anchor: Bounds<Pixels>,
        window: &Window,
        images: &mut BTreeMap<TabId, Vec<Arc<RenderImage>>>,
        cx: &Context<Self>,
    ) -> Option<AnyElement> {
        let state = self.state.borrow();
        let popup = state.extensions.popup(&self.id)?;
        let tab = popup.tab.clone();
        let title = popup.name.clone();
        drop(state);
        // Chromium's extension popup maximum is 800 by 600 DIPs (ExtensionPopup::kMaxSize).
        let available = window.viewport_size();
        let margin = px(tokens::MENU_WINDOW_MARGIN * 2.);
        let width = (available.width - margin).min(px(800.));
        let height = (available.height - margin - px(tokens::PILL_HEIGHT)).min(px(600.));
        let pane = self.pane(Owner::Popup(self.id.clone()), tab, images, cx);
        let panel = overlay_panel()
            .p_0()
            .gap_0()
            .w(width)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(tokens::SPACE_2))
                    .h(px(tokens::PILL_HEIGHT))
                    .child(title)
                    .child(
                        div()
                            .ml_auto()
                            .text_size(px(tokens::TEXT_SM))
                            .text_color(palette().text_2)
                            .child("Active-tab controls are unavailable in stock CEF"),
                    )
                    .child(
                        icon_button("close-extension-popup", "close", "Close extension").on_click(
                            cx.listener(|this, _: &ClickEvent, window, cx| {
                                this.dismiss_overlay(window, cx);
                            }),
                        ),
                    ),
            )
            .child(div().h(height).child(pane));
        Some(extension_overlay(anchor, panel, cx))
    }
}

fn extension_overlay(
    anchor: Bounds<Pixels>,
    panel: impl gpui::IntoElement,
    cx: &Context<Shell>,
) -> AnyElement {
    deferred(
        div()
            .absolute()
            .inset_0()
            .occlude()
            .child(div().absolute().inset_0().on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _: &MouseDownEvent, window, cx| {
                    this.dismiss_overlay(window, cx);
                    cx.stop_propagation();
                }),
            ))
            .child(
                anchored()
                    .attach(anchor, Anchor::BottomLeft)
                    .anchor(Anchor::TopLeft)
                    .snap_to_window_with_margin(px(tokens::MENU_WINDOW_MARGIN))
                    .child(panel),
            ),
    )
    .into_any_element()
}
