use anyhow::{Context as _, Result, ensure};
use flecha_adblock::site_host;
use flecha_cef::Partition;
use flecha_core::{TabId, WindowId, is_site_host};
use flecha_permissions::{Decision, Origin, PermissionKind, SiteSettings};
use gpui::{
    ClickEvent, Context, Div, ParentElement as _, StatefulInteractiveElement as _, Styled as _,
    Window, div, prelude::FluentBuilder as _, px, rgb,
};

use crate::permissions_view::permission_label;
use crate::view::{Overlay, Shell, button, overlay_panel};
use crate::{Action, Controller, dispatch};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Site {
    tab: TabId,
    partition: Partition,
    origin: Origin,
    host: Option<String>,
}

impl Site {
    fn from_url(tab: TabId, partition: Partition, url: &str) -> Option<Self> {
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
        }
    }

    fn site_permissions(&self, site: &Site) -> SiteSettings {
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

    pub(crate) fn site_controls_panel(&self, cx: &Context<Self>) -> Div {
        let state = self.state.borrow();
        let mut panel = overlay_panel().w(px(420.)).child(
            div()
                .flex()
                .justify_between()
                .items_center()
                .child(div().text_lg().child("Site Controls"))
                .child(button("close-site-controls", "Close").on_click(cx.listener(
                    |this, _: &ClickEvent, window, cx| this.dismiss_overlay(window, cx),
                ))),
        );
        let Some(site) = state.site_for_window(&self.id) else {
            return panel.child("Site Controls are available on web pages.");
        };
        panel = panel
            .child(div().truncate().child(site.origin.to_string()))
            .when(!state.status.is_empty(), |panel| {
                panel.child(state.status.clone())
            });
        if let Some(host) = &site.host {
            let settings = &state.model.core.settings;
            let shield_on = settings.blocks_ads_on(host);
            let global_on = settings.ad_block.unwrap_or(true);
            let blocked = state.engine.blocked_count(&site.tab);
            let toggle = button(
                "site-shield",
                if shield_on { "Turn off" } else { "Turn on" },
            )
            .on_click(self.site_click(&site, SiteChange::Shield(!shield_on), cx));
            panel = panel.child(
                div()
                    .flex()
                    .justify_between()
                    .items_center()
                    .child(
                        div()
                            .child(if shield_on {
                                "Site shield on"
                            } else {
                                "Site shield off"
                            })
                            .child(
                                div()
                                    .text_xs()
                                    .child(format!("{blocked} blocked on this page")),
                            )
                            .when(!global_on, |label| {
                                label.child("Ad blocking is off in Settings.")
                            }),
                    )
                    .child(toggle),
            );
        }
        if let Some(error) = state.permissions.error() {
            return panel.child(format!("Saved permissions are read-only. {error:#}"));
        }
        if !site.origin.is_potentially_trustworthy() {
            return panel.child("This origin cannot request site permissions.");
        }
        let permissions = state.site_permissions(&site);
        for kind in PermissionKind::ALL {
            let decision = permissions.decision(kind);
            panel = panel.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(permission_label(kind))
                    .child(
                        div().flex().gap_1().children(
                            [
                                (Decision::Ask, "Ask"),
                                (Decision::Allow, "Allow"),
                                (Decision::Block, "Block"),
                            ]
                            .map(|(value, label)| {
                                button(format!("permission-{}-{label}", kind.name()), label)
                                    .when(decision == value, |button| button.bg(rgb(0x27547b)))
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
        panel.child(
            button("clear-site-permissions", "Clear permissions").on_click(self.site_click(
                &site,
                SiteChange::ClearPermissions,
                cx,
            )),
        )
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
