use anyhow::{Context as _, Result, ensure};
use flecha_adblock::site_host;
use flecha_cef::{ConnectionSecurity, Copied, PageToolEvent, PageZoom, Partition, zoom_percentage};
use flecha_core::{Settings, TabId, WindowId, blocks_ads_on, is_site_host};
use flecha_permissions::{Decision, Origin, PermissionKind, SiteSettings};
use gpui::{
    AnyElement, ClickEvent, Context, Div, FontWeight, InteractiveElement as _, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};

use crate::icons::icon;
use crate::permissions_view::permission_label;
use crate::settings_view::SettingsTab;
use crate::shortcuts::Shortcut;
use crate::theme::palette;
use crate::tokens;
use crate::view::{
    Overlay, SHELL_SWITCH, Shell, danger_button, danger_icon_button, icon_button, is_dev_url,
    overlay_panel, popover_footer, site, switch,
};
use crate::{Action, Controller, dispatch};

/// The site panel's quick actions (ui/index.html:102-107).
#[derive(Clone, Copy)]
enum SiteTool {
    CopyLink,
    Screenshot,
    Boost,
    DevTools,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Site {
    tab: TabId,
    partition: Partition,
    origin: Origin,
    host: Option<String>,
}

impl Site {
    pub(crate) fn from_url(tab: TabId, partition: Partition, url: &str) -> Option<Self> {
        Some(Self {
            tab,
            partition,
            origin: Origin::parse(url).ok()?,
            host: site_host(url).filter(|host| is_site_host(host)),
        })
    }
}

#[derive(Clone)]
pub(crate) enum SiteChange {
    Shield(bool),
    Permission(PermissionKind, Decision),
    ClearPermissions,
    /// "Clear Site Data": the origin's cookies, storage and caches, then the page again.
    ClearData,
}

fn inherited_exception<'a>(settings: &'a Settings, host: &str) -> Option<&'a str> {
    settings
        .ad_block_allowed
        .as_ref()?
        .iter()
        .find(|site| {
            site.as_str() != host && !blocks_ads_on(Some(std::slice::from_ref(site)), host)
        })
        .map(String::as_str)
}

impl Controller {
    pub(crate) fn sync_adblock(&mut self) {
        let settings = &self.model.core.settings;
        self.engine
            .set_adblock_enabled(settings.ad_block.unwrap_or(true));
        self.engine
            .set_adblock_allowed_sites(settings.ad_block_allowed.as_deref().unwrap_or_default());
    }

    pub(crate) fn site_for_window(&self, window: &WindowId) -> Option<Site> {
        let (_, tab) = self.model.keyboard_pane(window)?;
        let partition = self.engine.partition(tab)?.clone();
        Site::from_url(tab.clone(), partition, &self.engine.url(tab)?)
    }

    pub(crate) fn change_current_site(
        &mut self,
        window: &WindowId,
        site: &Site,
        change: SiteChange,
    ) -> Result<()> {
        ensure!(
            self.site_for_window(window).as_ref() == Some(site),
            "The active site changed. Review its current Site Controls"
        );
        self.change_site(site, change)
    }

    fn change_site(&mut self, site: &Site, change: SiteChange) -> Result<()> {
        match change {
            SiteChange::Shield(block) => {
                let host = site
                    .host
                    .as_deref()
                    .context("No shield setting for this site")?;
                self.edit_settings(|settings| {
                    if block && let Some(parent) = inherited_exception(settings, host) {
                        anyhow::bail!("Change the exception for {parent} in Settings");
                    }
                    settings.set_ad_blocking_on(host, block);
                    Ok(())
                })
            }
            SiteChange::Permission(kind, decision) => {
                self.writable_permissions()?;
                self.engine
                    .set_site_permission(&site.partition, &site.origin, kind, decision)?;
                self.save_permissions()
            }
            SiteChange::ClearPermissions => {
                self.writable_permissions()?;
                self.engine
                    .clear_site_permissions(&site.partition, &site.origin);
                self.save_permissions()
            }
            SiteChange::ClearData => self.engine.clear_site_data(&site.tab, site.origin.as_str()),
        }
    }

    /// Reports the page tools' outcomes on the status line, and puts their pictures on the
    /// clipboard.
    pub(crate) fn poll_page_tools(&mut self, cx: &mut gpui::App) -> bool {
        let mut changed = false;
        while let Some(message) = self.engine.next_page_tool_event() {
            let text = match message.event {
                PageToolEvent::Copied {
                    what,
                    png: Some(png),
                } => {
                    let image = gpui::Image::from_bytes(gpui::ImageFormat::Png, png);
                    cx.write_to_clipboard(gpui::ClipboardItem::new_image(&image));
                    match what {
                        Copied::Screenshot => "Screenshot copied",
                        Copied::Image => "Image copied",
                    }
                }
                PageToolEvent::Copied { what, png: None } => match what {
                    Copied::Screenshot => "Could not take a screenshot of this page",
                    Copied::Image => "Could not copy this image",
                },
                PageToolEvent::SiteDataCleared { success: true } => "Site data cleared",
                PageToolEvent::SiteDataCleared { success: false } => {
                    "Could not clear this site's data"
                }
            };
            let session = self
                .model
                .partition(&message.tab)
                .and_then(|partition| partition.incognito_session());
            self.status.set(session, text);
            changed = true;
        }
        changed
    }

    pub(crate) fn site_permissions(&self, site: &Site) -> SiteSettings {
        self.engine.site_permissions(&site.partition, &site.origin)
    }
}

impl Shell {
    pub(crate) fn toggle_site_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::SiteControls) {
            self.dismiss_overlay(window, cx);
        } else if self.state.borrow().site_for_window(&self.id).is_some() {
            self.show_overlay(Overlay::SiteControls, window, cx);
        }
    }

    /// `.site-panel` (ui/styles.css:1962-2140): the site's name and connection, its ad shield,
    /// its permissions, and a way to clear them.
    pub(crate) fn site_controls_panel(&self, cx: &Context<Self>) -> Div {
        let state = self.state.borrow();
        let palette = palette();
        let inset = px(tokens::SITE_INSET);
        let mut panel = overlay_panel().p_0().gap_0();
        let Some(site) = state.site_for_window(&self.id) else {
            return panel.child(
                site_header(None, None, ConnectionSecurity::Unknown, None)
                    .child(close_site_controls(cx))
                    .child("Site Controls are available on web pages."),
            );
        };
        let origin = site.origin.to_string();
        let site_icon = self.site_icon(
            state.tab_favicon(&site.tab),
            &origin,
            (
                tokens::SITE_FAVICON,
                tokens::SITE_FAVICON_RADIUS,
                tokens::FAVICON_LETTER,
            ),
        );
        let security = state
            .engine
            .url(&site.tab)
            .map_or(ConnectionSecurity::Unknown, |url| {
                state.engine.connection_security(&site.tab, &url)
            });
        panel = panel.child(
            site_header(
                Some(&origin),
                Some(site_icon),
                security,
                state.status_for(&self.id),
            )
            .child(close_site_controls(cx)),
        );
        let mut body = div()
            .id("site-controls-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .overflow_y_scroll();
        let rule = |section: Div| {
            section
                .border_b(px(tokens::POPOVER_RULE))
                .border_color(palette.surface_line)
        };
        body = body.child(rule(self.site_actions(cx)));
        if let Some(host) = &site.host {
            let settings = &state.model.core.settings;
            let shield_on = settings.blocks_ads_on(host);
            let global_on = settings.ad_block.unwrap_or(true);
            let blocked = state.engine.blocked_count(&site.tab);
            let inherited = inherited_exception(settings, host);
            let note = |text: String| {
                div()
                    .text_size(px(tokens::TEXT_XS))
                    .text_color(palette.text_3)
                    .child(text)
            };
            // `.site-shield` (ui/styles.css:2052-2092).
            body = body.child(rule(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(tokens::SITE_SHIELD_GAP))
                    .py(px(tokens::SITE_SHIELD_PAD_Y))
                    .px(inset)
                    .child(icon(
                        "shield",
                        tokens::SITE_SHIELD_ICON,
                        if shield_on {
                            palette.green
                        } else {
                            palette.text_3
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(div().truncate().child(if shield_on {
                                "Site shield on"
                            } else {
                                "Site shield off"
                            }))
                            .child(note(format!("{blocked} blocked on this page")))
                            .when_some(inherited, |text, parent| {
                                text.child(note(format!(
                                    "Settings has an exception for {parent} and its subdomains."
                                )))
                            })
                            .when(!global_on, |text| {
                                text.child(note("Ad blocking is off in Settings.".into()))
                            }),
                    )
                    .when(inherited.is_none(), |row| {
                        row.child(
                            switch(
                                "site-shield",
                                shield_on,
                                if shield_on { "Turn off" } else { "Turn on" },
                                SHELL_SWITCH,
                            )
                            .on_click(self.site_click(
                                &site,
                                SiteChange::Shield(!shield_on),
                                cx,
                            )),
                        )
                    }),
            ));
        }
        let message = |text: String| {
            div()
                .flex_none()
                .px(inset)
                .py(px(tokens::SITE_SHIELD_PAD_Y))
                .child(text)
        };
        let zoom = state
            .engine
            .zoom_level(&site.tab)
            .map_or(100, zoom_percentage);
        let finish = |body: gpui::Stateful<Div>, permissions: bool| {
            panel.child(body.child(self.zoom_row(zoom, cx)).child(self.site_footer(
                &site,
                permissions,
                cx,
            )))
        };
        if let Some(error) = state.permissions.error() {
            return finish(
                body.child(message(format!(
                    "Saved permissions are read-only. {error:#}"
                ))),
                false,
            );
        }
        if let Some(error) = state.permissions.save_error() {
            body = body.child(
                message(format!(
                    "Permission changes are active in this session. Could not save them. {error:#}"
                ))
                .child(self.action_button(
                    "retry-permission-save",
                    "Retry save",
                    Action::RetryPermissionSave,
                    cx,
                )),
            );
        }
        if !site.origin.is_potentially_trustworthy() {
            return finish(
                body.child(message(
                    "This origin cannot request site permissions.".into(),
                )),
                false,
            );
        }
        // `.site-label` and `.field-label` (ui/styles.css:1450-1455, 2094-2097).
        body = body.child(
            div()
                .flex_none()
                .pt(px(tokens::SITE_LABEL_PAD_TOP))
                .px(inset)
                .pb(px(tokens::SITE_LABEL_PAD_BOTTOM))
                .text_size(px(tokens::TEXT_XS))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.text_3)
                .child("Permissions"),
        );
        let permissions = state.site_permissions(&site);
        for kind in PermissionKind::ALL {
            let decision = permissions.decision(kind);
            // `.site-row` (ui/styles.css:2099-2110), its choice in `.mini-select`'s chip
            // (ui/styles.css:2112-2121).
            body = body.child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .gap(px(tokens::SITE_ROW_GAP))
                    .h(px(tokens::SITE_ROW))
                    .px(inset)
                    .child(div().min_w_0().truncate().child(permission_label(kind)))
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .h(px(tokens::MINI_SELECT))
                            .rounded(px(tokens::RADIUS_SM))
                            .bg(palette.surface_2)
                            .text_size(px(tokens::TEXT_SM))
                            .font_weight(FontWeight::MEDIUM)
                            .children(
                                [
                                    (Decision::Ask, "Ask"),
                                    (Decision::Allow, "Allow"),
                                    (Decision::Block, "Block"),
                                ]
                                .map(|(value, label)| {
                                    div()
                                        .id(format!("permission-{}-{label}", kind.name()))
                                        .flex()
                                        .items_center()
                                        .h_full()
                                        .px(px(tokens::MINI_SELECT_PAD))
                                        .rounded(px(tokens::RADIUS_SM))
                                        .cursor_pointer()
                                        .map(|choice| {
                                            if decision == value {
                                                choice
                                                    .bg(palette.selection)
                                                    .text_color(palette.selection_text)
                                            } else {
                                                choice
                                                    .text_color(palette.text_2)
                                                    .hover(|choice| choice.text_color(palette.text))
                                            }
                                        })
                                        .child(label)
                                        .on_click(self.site_click(
                                            &site,
                                            SiteChange::Permission(kind, value),
                                            cx,
                                        ))
                                }),
                            ),
                    ),
            );
        }
        finish(body, true)
    }

    /// `.site-actions` (ui/styles.css:2007-2047, ui/index.html:102-107): Copy Link, Screenshot,
    /// Boost and DevTools in two columns. Each closes the panel first, as ui/app.ts:912-928 does.
    fn site_actions(&self, cx: &Context<Self>) -> Div {
        let palette = palette();
        let inset = px(tokens::SITE_INSET);
        let action =
            |id: &'static str, glyph: &'static str, label: &'static str, tool: SiteTool| {
                let group = gpui::SharedString::new_static("site-action");
                div()
                    .id(id)
                    .group(group.clone())
                    .flex()
                    .items_center()
                    .gap(px(tokens::SITE_ACTION_GAP))
                    .h(px(tokens::SITE_ACTION))
                    .min_w_0()
                    .px(px(tokens::SITE_ACTION_PAD))
                    .rounded(px(tokens::RADIUS_MD))
                    .bg(palette.surface_2)
                    .text_size(px(tokens::TEXT_SM))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(palette.text_2)
                    .cursor_pointer()
                    .hover(|button| {
                        button
                            .bg(palette.fill_hover)
                            .text_color(palette.text)
                            .shadow(vec![crate::view::hairline(palette.surface_line)])
                    })
                    .child(
                        icon(glyph, tokens::SITE_ACTION_ICON, palette.text_2)
                            .flex_none()
                            .group_hover(group, |glyph| glyph.text_color(palette.text)),
                    )
                    .child(div().min_w_0().truncate().child(label))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.run_site_tool(tool, window, cx);
                    }))
            };
        div()
            .grid()
            .grid_cols(2)
            .flex_none()
            .gap(px(tokens::SITE_ACTIONS_GAP))
            .px(inset)
            .pb(inset)
            .child(action("site-copy", "link", "Copy Link", SiteTool::CopyLink))
            .child(action(
                "site-shot",
                "camera",
                "Screenshot",
                SiteTool::Screenshot,
            ))
            .child(action("site-boost", "zap", "Boost", SiteTool::Boost))
            .child(action(
                "site-devtools",
                "wrench",
                "DevTools",
                SiteTool::DevTools,
            ))
    }

    /// The Zoom `.site-row` with its `.zoom-control` (ui/index.html:112-119,
    /// ui/styles.css:2123-2150): out, the level (which resets it), in.
    fn zoom_row(&self, percent: u16, cx: &Context<Self>) -> Div {
        let palette = palette();
        let step = |id: &'static str, glyph: &'static str, label: &'static str, zoom: PageZoom| {
            crate::view::tinted_icon_button(
                id,
                glyph,
                label,
                (tokens::ZOOM_STEP, tokens::ZOOM_STEP_ICON),
                palette.text_2,
            )
            .h(px(tokens::ZOOM_VALUE))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                this.zoom(zoom, cx);
            }))
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .gap(px(tokens::SITE_ROW_GAP))
            .h(px(tokens::SITE_ROW))
            .px(px(tokens::SITE_INSET))
            .child(div().min_w_0().truncate().child("Zoom"))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .w(px(tokens::SITE_CONTROL))
                    .h(px(tokens::ZOOM_VALUE))
                    .rounded(px(tokens::RADIUS_SM))
                    .bg(palette.surface_2)
                    .child(step("zoom-out", "minus", "Zoom out", PageZoom::Out))
                    .child(
                        div()
                            .id("zoom-reset")
                            .flex()
                            .flex_1()
                            .items_center()
                            .justify_center()
                            .h_full()
                            .text_size(px(tokens::TEXT_XS))
                            .font_family(tokens::MONO)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(palette.text_2)
                            .cursor_pointer()
                            .hover(|value| value.text_color(palette.text))
                            .aria_label("Actual size")
                            .child(format!("{percent}%"))
                            .on_click(cx.listener(|this, _: &ClickEvent, _, cx| {
                                this.zoom(PageZoom::Reset, cx);
                            })),
                    )
                    .child(step("zoom-in", "plus", "Zoom in", PageZoom::In)),
            )
    }

    /// `.site-panel .popover-footer` (ui/styles.css:2152-2167): Clear Site Data, and the
    /// rewrite's Clear permissions while permissions can be saved.
    fn site_footer(&self, site: &Site, permissions: bool, cx: &Context<Self>) -> Div {
        popover_footer()
            .mt(px(tokens::SITE_FOOTER_MARGIN))
            .pt(px(tokens::SITE_FOOTER_PAD_TOP))
            .px(px(tokens::SITE_INSET))
            .pb(px(tokens::SITE_FOOTER_PAD_BOTTOM))
            .child(
                danger_icon_button("site-clear", "trash", "Clear Site Data")
                    .on_click(self.site_click(site, SiteChange::ClearData, cx)),
            )
            .when(permissions, |footer| {
                footer.child(
                    danger_button("clear-site-permissions", "Clear permissions")
                        .on_click(self.site_click(site, SiteChange::ClearPermissions, cx)),
                )
            })
    }

    /// Runs a site action on the shown page, after closing the panel.
    fn run_site_tool(&mut self, tool: SiteTool, window: &mut Window, cx: &mut Context<Self>) {
        self.dismiss_overlay(window, cx);
        let shown = {
            let state = self.state.borrow();
            state
                .model
                .keyboard_pane(&self.id)
                .map(|(_, tab)| (tab.clone(), state.engine.url(tab)))
        };
        let Some((tab, url)) = shown else {
            return;
        };
        let result = match tool {
            SiteTool::CopyLink => {
                if let Some(url) = url {
                    self.copy_text(url, "Link copied", cx);
                }
                return;
            }
            SiteTool::Boost => {
                self.invoke_shortcut(Shortcut::SettingsTab(SettingsTab::Boosts), window, cx);
                return;
            }
            SiteTool::DevTools => {
                self.invoke_shortcut(Shortcut::DevTools, window, cx);
                return;
            }
            SiteTool::Screenshot => self.state.borrow().engine.capture_screenshot(&tab),
        };
        if let Err(error) = result {
            let mut state = self.state.borrow_mut();
            let session = state.model.incognito_session(&self.id);
            state.status.set(session, format!("{error:#}"));
            cx.notify();
        }
    }

    fn site_click(
        &self,
        site: &Site,
        change: SiteChange,
        cx: &Context<Self>,
    ) -> impl Fn(&ClickEvent, &mut Window, &mut gpui::App) + 'static {
        let action = Action::Site(site.clone(), change);
        cx.listener(move |this, _: &ClickEvent, _, cx| {
            dispatch(&this.state, &this.id, action.clone(), cx);
        })
    }
}

/// `.site-header` (ui/styles.css:1972-2004): the site's icon, its host, and how the page
/// connects, worded as ui/app.ts:850-857 words it.
fn site_header(
    origin: Option<&str>,
    icon: Option<AnyElement>,
    connection: ConnectionSecurity,
    status: Option<&str>,
) -> Div {
    let palette = palette();
    let inset = px(tokens::SITE_INSET);
    let insecure = matches!(
        connection,
        ConnectionSecurity::CertificateError | ConnectionSecurity::InsecureContent
    ) || origin.is_some_and(|url| url.starts_with("http:") && !is_dev_url(url));
    let security = origin.map(|url| {
        if connection == ConnectionSecurity::CertificateError {
            "Certificate could not be verified"
        } else if connection == ConnectionSecurity::InsecureContent {
            "Some content loaded over an insecure connection"
        } else if connection == ConnectionSecurity::VerifiedHttps {
            "Secure connection"
        } else if is_dev_url(url) {
            "Local development server"
        } else if url.starts_with("https:") {
            "HTTPS status unavailable"
        } else if url.starts_with("http:") {
            "Connection is not secure"
        } else {
            "Not a web page"
        }
    });
    let small = |text: String, color| {
        div()
            .text_size(px(tokens::TEXT_XS))
            .text_color(color)
            .child(text)
    };
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(tokens::SITE_HEADER_GAP))
        .pt(inset)
        .px(inset)
        .pb(px(tokens::SITE_HEADER_PAD_BOTTOM))
        .children(icon)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(origin.map_or("Site Controls", site).to_owned()),
                )
                .children(security.map(|text| {
                    small(
                        text.to_owned(),
                        if insecure {
                            palette.amber
                        } else {
                            palette.text_3
                        },
                    )
                }))
                .children(status.map(|text| small(text.to_owned(), palette.text_3))),
        )
}

/// The rewrite's Close button; the shipping panel closes when a click lands outside it.
fn close_site_controls(cx: &Context<Shell>) -> gpui::Stateful<Div> {
    icon_button("close-site-controls", "close", "Close Site Controls")
        .on_click(cx.listener(|this, _: &ClickEvent, window, cx| this.dismiss_overlay(window, cx)))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use flecha_adblock::{Request, RequestType};

    use super::*;

    fn site() -> Site {
        Site::from_url(
            TabId::new("tab"),
            Partition::Shared,
            "https://www.example.test/page",
        )
        .unwrap()
    }

    #[test]
    fn site_controls_save_load_and_clear_origin_permissions() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let site = site();
        let mut controller = Controller::open_test_profile(profile.path())?;
        let camera = PermissionKind::Camera;
        let microphone = PermissionKind::Microphone;
        assert_eq!(
            controller.site_permissions(&site).decision(camera),
            Decision::Ask
        );
        controller.change_site(&site, SiteChange::Permission(camera, Decision::Allow))?;
        controller.change_site(&site, SiteChange::Permission(microphone, Decision::Block))?;
        assert!(!controller.engine.permissions_dirty());
        assert!(profile.path().join("permissions.json").is_file());
        drop(controller);

        let mut restarted = Controller::open_test_profile(profile.path())?;
        let saved = restarted.site_permissions(&site);
        assert_eq!(saved.decision(camera), Decision::Allow);
        assert_eq!(saved.decision(microphone), Decision::Block);
        restarted.change_site(&site, SiteChange::Permission(camera, Decision::Ask))?;
        assert_eq!(
            restarted.site_permissions(&site).decision(camera),
            Decision::Ask
        );
        assert_eq!(
            restarted.site_permissions(&site).decision(microphone),
            Decision::Block
        );
        restarted.change_site(&site, SiteChange::ClearPermissions)?;
        assert!(!restarted.engine.permissions_dirty());
        drop(restarted);
        let reopened = Controller::open_test_profile(profile.path())?;
        assert_eq!(
            reopened.site_permissions(&site).decision(microphone),
            Decision::Ask
        );
        Ok(())
    }

    #[test]
    fn corrupt_and_unreadable_permissions_stay_in_place() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let path = profile.path().join("permissions.json");
        let corrupt = br#"{"version":1,"partitions":"corrupt"}"#;
        fs::write(&path, corrupt)?;
        let site = site();
        let mut controller = Controller::open_test_profile(profile.path())?;
        assert!(controller.permissions.error().is_some());
        assert!(
            controller
                .change_site(
                    &site,
                    SiteChange::Permission(PermissionKind::Camera, Decision::Allow)
                )
                .is_err()
        );
        controller.engine.set_site_permission(
            &site.partition,
            &site.origin,
            PermissionKind::Camera,
            Decision::Block,
        )?;
        controller.poll_permissions();
        assert!(controller.engine.permissions_dirty());
        assert_eq!(fs::read(&path)?, corrupt);

        let unreadable = tempfile::tempdir()?;
        let path = unreadable.path().join("permissions.json");
        fs::create_dir(&path)?;
        let mut controller = Controller::open_test_profile(unreadable.path())?;
        assert!(controller.permissions.error().is_some());
        assert!(
            controller
                .change_site(&site, SiteChange::ClearPermissions)
                .is_err()
        );
        assert!(path.is_dir());
        Ok(())
    }

    #[test]
    fn a_failed_permission_revocation_can_be_retried() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let site = site();
        let permission = PermissionKind::Camera;
        let mut controller = Controller::open_test_profile(profile.path())?;
        controller.change_site(&site, SiteChange::Permission(permission, Decision::Allow))?;

        let lock = profile.path().join("permissions.json.lock");
        fs::remove_file(&lock)?;
        fs::create_dir(&lock)?;
        assert!(
            controller
                .change_site(&site, SiteChange::ClearPermissions)
                .is_err()
        );
        assert!(controller.permissions.save_error().is_some());
        assert!(controller.engine.permissions_dirty());
        assert_eq!(
            controller.site_permissions(&site).decision(permission),
            Decision::Ask
        );
        let saved = fs::read_to_string(profile.path().join("permissions.json"))?;
        assert!(saved.contains(r#""camera": "allow""#));

        fs::remove_dir(&lock)?;
        let window = controller.model.core.focused_window().id.clone();
        controller.act(&window, Action::RetryPermissionSave)?;
        assert!(controller.permissions.save_error().is_none());
        assert!(!controller.engine.permissions_dirty());
        let reloaded = Controller::open_test_profile(profile.path())?;
        assert_eq!(
            reloaded.site_permissions(&site).decision(permission),
            Decision::Ask
        );
        Ok(())
    }

    #[test]
    fn a_subdomain_shield_cannot_remove_a_parent_exception() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let mut controller = Controller::open_test_profile(profile.path())?;
        controller.edit_settings(|settings| {
            settings.set_ad_blocking_on("example.test", false);
            Ok(())
        })?;
        assert!(
            controller
                .change_site(&site(), SiteChange::Shield(true))
                .is_err()
        );
        assert_eq!(controller.engine.adblock_allowed_sites(), ["example.test"]);
        Ok(())
    }

    #[test]
    fn shield_and_settings_drive_the_engine_filter_and_survive_restart() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let cache = profile.path().join("adblock");
        fs::create_dir(&cache)?;
        fs::write(
            cache.join("easylist.txt"),
            "[Adblock Plus 2.0]\n! Title: EasyList\n\n||ads.example.test^\n",
        )?;
        fs::write(
            cache.join("easyprivacy.txt"),
            "[Adblock Plus 2.0]\n! Title: EasyPrivacy\n\n||tracker.example.test^\n",
        )?;
        let mut controller = Controller::open_test_profile(profile.path())?;
        controller.engine.load_adblock_rules(&cache);
        let site = site();
        let request = Request {
            url: "https://ads.example.test/ad.js",
            source_url: site.origin.as_str(),
            top_level_url: site.origin.as_str(),
            frame_parent_url: None,
            request_type: RequestType::Script,
        };
        assert!(controller.engine.adblocker().should_block(&request));
        controller.change_site(&site, SiteChange::Shield(false))?;
        assert!(!controller.engine.adblocker().should_block(&request));
        assert_eq!(
            controller.engine.adblock_allowed_sites(),
            ["www.example.test"]
        );
        drop(controller);

        let mut restarted = Controller::open_test_profile(profile.path())?;
        restarted.engine.load_adblock_rules(&cache);
        assert!(!restarted.engine.adblocker().should_block(&request));
        restarted.change_site(&site, SiteChange::Shield(true))?;
        assert!(restarted.engine.adblocker().should_block(&request));
        restarted.edit_settings(|settings| {
            settings.ad_block = Some(false);
            Ok(())
        })?;
        assert!(!restarted.engine.adblocker().should_block(&request));
        Ok(())
    }
}
