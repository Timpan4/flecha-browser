//! Flechinha's toolbar (chromium/shell/ui/flechinha.html:16-44, flechinha.css, flechinha.ts):
//! the window controls, Back, Forward and Reload, the address, which turns into a field on a
//! click or ⌘L, the Copy link button inside it, and Open in Flecha.

use std::time::Duration;

use flecha_core::suggestions::{InputResolution, resolve_input};
use gpui::prelude::FluentBuilder as _;
use gpui::{
    Animation, AnimationExt as _, ClickEvent, Context, Div, FontWeight, InteractiveElement as _,
    IntoElement, Keystroke, Modifiers, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, relative,
};

use crate::icons::icon;
use crate::model::Transient;
use crate::shortcuts::Navigation;
use crate::text_field::TextField;
use crate::theme::palette;
use crate::tokens::{self, ease};
use crate::view::{Overlay, Shell, hairline, is_dev_url, site, tinted_icon_button};
use crate::window_chrome::{TitleBar, close_control};
use crate::{Action, dispatch};

impl Shell {
    /// The toolbar above a Flechinha's page. Its empty room moves the window.
    pub(crate) fn flechinha_toolbar(
        &self,
        page: &Transient,
        title_bar: TitleBar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let toolbar = self
            .state
            .borrow()
            .toolbars
            .get(&self.id)
            .copied()
            .unwrap_or_default();
        let empty = page.tab.url.is_empty() || page.tab.url == "about:blank";
        // `.icon-button` (ui/flechinha.css:87-110): 28 square, a 16 glyph, faded while it can't run.
        let sizes = (
            tokens::ICON_BUTTON,
            tokens::ICON,
            tokens::FLECHINHA_DISABLED_OPACITY,
        );
        let reload = if toolbar.loading {
            self.sized_navigation_button(("reload", "close"), Navigation::Stop, true, sizes, cx)
        } else {
            self.sized_navigation_button(
                ("reload", "reload"),
                Navigation::Reload,
                !empty,
                sizes,
                cx,
            )
        };
        let bar = div()
            .h(px(tokens::FLECHINHA_TOOLBAR))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(tokens::TOOLBAR_GAP))
            .pl(px(tokens::SPACE_2 - tokens::TOOLBAR_GAP))
            .pr(px(tokens::SPACE_2))
            .map(|bar| match title_bar {
                TitleBar::Platform => bar.child(close_control().on_click(cx.listener(
                    |this, _: &ClickEvent, _, cx| {
                        dispatch(&this.state, &this.id, Action::Close, cx);
                    },
                ))),
                _ => bar.children(self.window_controls(
                    title_bar,
                    (
                        tokens::FLECHINHA_WINDOW_CONTROLS_WIDTH,
                        tokens::FLECHINHA_WINDOW_CONTROLS_PAD_LEFT,
                    ),
                    window,
                    cx,
                )),
            })
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(tokens::TOOLBAR_GAP))
                    .occlude()
                    .child(self.sized_navigation_button(
                        ("back", "arrowLeft"),
                        Navigation::Back,
                        toolbar.back,
                        sizes,
                        cx,
                    ))
                    .child(self.sized_navigation_button(
                        ("forward", "arrowRight"),
                        Navigation::Forward,
                        toolbar.forward,
                        sizes,
                        cx,
                    ))
                    .child(reload),
            )
            .child(self.flechinha_address(page, toolbar.loading, empty, cx))
            .children(
                self.zoom_badge(cx)
                    .map(|badge| div().flex_none().occlude().child(badge)),
            )
            .child(
                div()
                    .id("return")
                    .occlude()
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
            );
        self.window_drag(bar, title_bar, cx)
    }

    /// `#address` (ui/flechinha.css:114-284): the site, its host in bold and the page's title,
    /// or the field while it is edited; Copy link shows while the pointer is over it, and a
    /// line runs along its bottom while the page loads.
    fn flechinha_address(
        &self,
        page: &Transient,
        loading: bool,
        empty: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        let palette = palette();
        let group = SharedString::new_static("flechinha-address");
        let editing = match &self.overlay {
            Overlay::FlechinhaAddress(input) => Some(input),
            _ => None,
        };
        let flashing = self.state.borrow().copy_link_flashes.contains_key(&self.id);
        let url = page.tab.url.as_str();
        let title =
            Some(page.tab.title.as_str()).filter(|title| !title.is_empty() && *title != url);
        let address = div()
            .id("flechinha-address")
            .group(group.clone())
            .relative()
            .flex()
            .items_center()
            .flex_1()
            .min_w_0()
            .h(px(tokens::PILL_HEIGHT))
            .mx(px(tokens::FLECHINHA_ADDRESS_MARGIN))
            .pr(px(tokens::FLECHINHA_ADDRESS_PAD_RIGHT))
            .rounded(px(tokens::PAGE_RADIUS))
            .overflow_hidden()
            .occlude();
        let address = match editing {
            // `#address.editing` and `#address-edit` (ui/flechinha.css:140-143, 208-237).
            Some(input) => address
                .bg(palette.surface)
                .shadow(tokens::shadow_address_editing(palette.dark))
                .child(
                    div()
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .items_center()
                        .gap(px(tokens::SPACE_2))
                        .pl(px(tokens::FLECHINHA_ADDRESS_SHOW_PAD_LEFT))
                        .pr(px(tokens::FLECHINHA_EDIT_PAD_RIGHT))
                        .text_color(palette.text_3)
                        .child(icon("search", tokens::FLECHINHA_SITE_GLYPH, palette.text_3))
                        .child(input.render_inline("Search or enter address", true)),
                ),
            None => address
                .bg(palette.fill_subtle)
                .shadow(vec![hairline(palette.line)])
                .hover(|address| address.bg(palette.fill_hover))
                .child(
                    div()
                        .id("address-show")
                        .flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .items_center()
                        .gap(px(tokens::SPACE_2))
                        .pl(px(tokens::FLECHINHA_ADDRESS_SHOW_PAD_LEFT))
                        .pr(px(tokens::FLECHINHA_ADDRESS_SHOW_PAD_RIGHT))
                        .cursor_pointer()
                        .aria_label("Edit address")
                        .when(flashing, |show| show.opacity(0.))
                        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| {
                            this.edit_flechinha_address(window, cx);
                        }))
                        .child(self.flechinha_site_icon(page))
                        .child(
                            div()
                                .flex_none()
                                .max_w(relative(tokens::FLECHINHA_HOST_MAX_WIDTH / 100.))
                                .truncate()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(site(url).to_owned()),
                        )
                        .when_some(title, |show, title| {
                            show.child(
                                div()
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .text_color(palette.text_3)
                                    .child(
                                        div()
                                            .flex_none()
                                            .mr(px(tokens::FLECHINHA_TITLE_DASH_GAP))
                                            .opacity(tokens::FLECHINHA_TITLE_DASH_OPACITY)
                                            .child("—"),
                                    )
                                    .child(div().min_w_0().truncate().child(title.to_owned())),
                            )
                        }),
                )
                .when(!empty && !flashing, |address| {
                    address.child(
                        tinted_icon_button(
                            "copy-link",
                            "link",
                            "Copy link",
                            (
                                tokens::FLECHINHA_ADDRESS_ACTION,
                                tokens::FLECHINHA_ADDRESS_ACTION_ICON,
                            ),
                            palette.text_3,
                        )
                        .invisible()
                        .group_hover(group, |action| action.visible())
                        .on_click(cx.listener(
                            |this, _: &ClickEvent, _, cx| {
                                this.copy_link(cx);
                            },
                        )),
                    )
                })
                .when(flashing, |address| {
                    address.child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("✓  Link copied"),
                    )
                }),
        };
        address.when(loading, |address| {
            // `#address.loading::before` (ui/flechinha.css:273-293): a 30% line sliding from
            // just left of the address to past its right edge.
            address.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .items_end()
                    .child(
                        div()
                            .h(px(tokens::FLECHINHA_LOADING))
                            .w(relative(tokens::FLECHINHA_LOADING_WIDTH / 100.))
                            .rounded(px(tokens::FLECHINHA_LOADING))
                            .bg(palette.blue)
                            .with_animation(
                                "flechinha-loading",
                                Animation::new(Duration::from_secs_f32(
                                    tokens::FLECHINHA_LOADING_SECONDS,
                                ))
                                .repeat()
                                .with_easing(ease),
                                |line, delta| {
                                    // translateX from -100% to the travel, in the line's widths.
                                    let share = tokens::FLECHINHA_LOADING_WIDTH / 100.;
                                    let to = tokens::FLECHINHA_LOADING_TRAVEL / 100.;
                                    line.ml(relative(share * (-1. + (to + 1.) * delta)))
                                },
                            ),
                    ),
            )
        })
    }

    /// `#site-icon` (ui/flechinha.css:156-180, flechinha.ts:55-65): the page's icon, an open
    /// lock in amber on plain HTTP, or a globe.
    fn flechinha_site_icon(&self, page: &Transient) -> gpui::AnyElement {
        let url = page.tab.url.as_str();
        let insecure = url.starts_with("http:") && !is_dev_url(url);
        let frame = || {
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(tokens::FLECHINHA_SITE_ICON))
        };
        if insecure {
            return frame()
                .child(icon(
                    "unlocked",
                    tokens::FLECHINHA_SITE_GLYPH,
                    palette().amber,
                ))
                .into_any_element();
        }
        match self.state.borrow().tab_favicon(&self.id, &page.id) {
            Some(image) => self.site_icon(
                Some(image),
                url,
                (
                    tokens::FLECHINHA_SITE_ICON,
                    tokens::FLECHINHA_SITE_ICON_RADIUS,
                    0.,
                ),
            ),
            None => frame()
                .child(icon(
                    "globe",
                    tokens::FLECHINHA_SITE_GLYPH,
                    palette().text_3,
                ))
                .into_any_element(),
        }
    }

    /// Turns a Flechinha's address into a field holding the page's address, all selected
    /// (`startEditing` in ui/flechinha.ts:97-106). Returns whether this window is a Flechinha.
    pub(crate) fn edit_flechinha_address(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let url = {
            let state = self.state.borrow();
            let Some(page) = state.model.flechinhas.get(&self.id) else {
                return false;
            };
            page.tab.url.clone()
        };
        self.show_overlay(
            Overlay::FlechinhaAddress(TextField::new(url, true)),
            window,
            cx,
        );
        true
    }

    /// The address field's keys: Return goes to the address or searches for the text
    /// (ui/flechinha.ts:120-129); Escape reaches `dismiss_overlay` as a shortcut.
    pub(crate) fn flechinha_address_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Overlay::FlechinhaAddress(input) = &mut self.overlay else {
            return;
        };
        if stroke.key != "enter" || stroke.modifiers != Modifiers::none() {
            input.key(stroke, cx);
            cx.notify();
            return;
        }
        let engine = self.state.borrow().model.core.settings.search_engine;
        let url = match resolve_input(input.text(), engine) {
            InputResolution::Navigate(url) | InputResolution::Search(url) => url,
            InputResolution::Invalid => return,
        };
        self.dismiss_overlay(window, cx);
        dispatch(&self.state, &self.id, Action::Navigate(url), cx);
    }
}
