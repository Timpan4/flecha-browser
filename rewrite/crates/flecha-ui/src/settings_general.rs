use std::path::Path;

use anyhow::{Context as _, Result, ensure};
use flecha_core::{
    ArchivePolicy, Environment, PermissionDecision, SearchEngine, Settings, SidebarWidth, Theme,
    is_site_host,
    suggestions::{format_targets, parse_targets},
};
use gpui::{
    ClickEvent, Context, Div, ElementId, ParentElement as _, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _, rgb,
};
use uuid::Uuid;

use super::{Draft, Field, SettingsPanel, muted, row, section};
use crate::view::{Shell, button};

#[derive(Clone, PartialEq, Eq)]
pub(super) enum GeneralField {
    Width,
    ArchiveHours,
    DownloadFolder,
    AdException,
    Environment(Option<String>),
}

impl GeneralField {
    fn id(&self) -> ElementId {
        match self {
            Self::Width => "sidebar-width".into(),
            Self::ArchiveHours => "archive-hours".into(),
            Self::DownloadFolder => "downloads-folder".into(),
            Self::AdException => "ad-exception".into(),
            Self::Environment(None) => "environment-add".into(),
            Self::Environment(Some(id)) => format!("environment-{id}").into(),
        }
    }

    fn placeholder(&self) -> &'static str {
        match self {
            Self::Width => "Pixels",
            Self::ArchiveHours => "Hours (0 is Never)",
            Self::DownloadFolder => "Absolute path, or leave empty for the default",
            Self::AdException => "example.com",
            Self::Environment(_) => "Project name, then one target per line",
        }
    }

    fn apply(&self, settings: &mut Settings, text: &str) -> Result<()> {
        let text = text.trim();
        match self {
            Self::Width => {
                let width: f64 = text.parse().context("Enter a sidebar width in pixels")?;
                ensure!(width.is_finite(), "Enter a finite sidebar width");
                settings.sidebar_width =
                    SidebarWidth::clamp(width).context("Invalid sidebar width")?;
            }
            Self::ArchiveHours => {
                let hours = text.parse().context("Enter the number of hours")?;
                settings.archive_policy = ArchivePolicy::from_hours(hours)
                    .context("Enter zero or a positive number of hours")?;
            }
            Self::DownloadFolder => {
                ensure!(
                    text.is_empty() || Path::new(text).is_absolute(),
                    "Enter an absolute path"
                );
                settings.downloads_folder = (!text.is_empty()).then(|| text.to_owned());
            }
            Self::AdException => {
                let host = text.to_ascii_lowercase();
                ensure!(is_site_host(&host), "Enter a hostname such as example.com");
                settings.set_ad_blocking_on(&host, false);
            }
            Self::Environment(id) => {
                let (name, input) = text
                    .split_once('\n')
                    .context("Enter a project name and at least one target")?;
                let name = name.trim();
                let targets = parse_targets(input);
                ensure!(!name.is_empty(), "Enter a project name");
                ensure!(
                    !targets.is_empty()
                        && targets.len()
                            == input.lines().filter(|line| !line.trim().is_empty()).count(),
                    "Enter one web address per target line, with an optional label before the address"
                );
                if let Some(id) = id {
                    let project = settings
                        .environments
                        .iter_mut()
                        .find(|project| project.id == *id)
                        .context("This project was removed")?;
                    project.name = name.into();
                    project.targets = targets;
                } else {
                    settings.environments.push(Environment {
                        id: Uuid::new_v4().to_string(),
                        name: name.into(),
                        targets,
                    });
                }
            }
        }
        Ok(())
    }
}

impl Shell {
    pub(super) fn general_settings(&self, panel: &SettingsPanel, cx: &Context<Self>) -> Div {
        let state = self.state.borrow();
        let settings = &state.model.core.settings;
        let mut search = row();
        for engine in [
            SearchEngine::Google,
            SearchEngine::DuckDuckGo,
            SearchEngine::Bing,
            SearchEngine::Brave,
            SearchEngine::Kagi,
            SearchEngine::Ecosia,
        ] {
            search = search.child(self.preference(
                engine.name(),
                engine.name(),
                settings.search_engine == engine,
                move |settings| settings.search_engine = engine,
                cx,
            ));
        }
        let mut themes = row();
        for (theme, name) in [
            (Theme::System, "System"),
            (Theme::Light, "Light"),
            (Theme::Dark, "Dark"),
        ] {
            themes = themes.child(self.preference(
                name,
                name,
                settings.theme == theme,
                move |settings| settings.theme = theme,
                cx,
            ));
        }
        let appearance = section("Appearance")
            .child(themes)
            .child(self.edit_value(
                panel,
                GeneralField::Width,
                "Sidebar width",
                settings.sidebar_width.pixels().to_string(),
                cx,
            ))
            .child(self.toggle(
                "fullscreen-sidebar",
                "Sidebar in page fullscreen",
                settings.sidebar_in_page_fullscreen.unwrap_or(false),
                |settings, enabled| settings.sidebar_in_page_fullscreen = Some(enabled),
                cx,
            ));

        let mut archive = row();
        for (policy, label) in [
            (ArchivePolicy::HALF_DAY, "12 hours"),
            (ArchivePolicy::DAY, "1 day"),
            (ArchivePolicy::WEEK, "1 week"),
            (ArchivePolicy::THIRTY_DAYS, "30 days"),
            (ArchivePolicy::NEVER, "Never"),
        ] {
            archive = archive.child(self.preference(
                label,
                label,
                settings.archive_policy == policy,
                move |settings| settings.archive_policy = policy,
                cx,
            ));
        }
        let tabs = section("Tabs")
            .child(self.toggle(
                "peek-pinned",
                "Open links from pinned tabs in Peek",
                settings.peek_pinned_links.unwrap_or(true),
                |settings, enabled| settings.peek_pinned_links = Some(enabled),
                cx,
            ))
            .child(self.toggle(
                "links-flechinha",
                "Open external links in Flechinha",
                settings.open_links_in_flechinha.unwrap_or(true),
                |settings, enabled| settings.open_links_in_flechinha = Some(enabled),
                cx,
            ))
            .child(self.edit_value(
                panel,
                GeneralField::ArchiveHours,
                "Auto-archive after (hours)",
                settings.archive_policy.hours().to_string(),
                cx,
            ))
            .child(archive);

        let mut ad_block = section("Ad blocking").child(self.toggle(
            "ad-block",
            "Block ads",
            settings.ad_block.unwrap_or(true),
            |settings, enabled| settings.ad_block = Some(enabled),
            cx,
        ));
        for host in settings.ad_block_allowed.iter().flatten() {
            ad_block = ad_block.child(row().justify_between().child(muted(host.clone())).child(
                self.preference(
                    format!("remove-exception-{host}"),
                    "Remove exception",
                    false,
                    {
                        let host = host.clone();
                        move |settings| {
                            if let Some(sites) = &mut settings.ad_block_allowed {
                                sites.retain(|site| *site != host);
                            }
                        }
                    },
                    cx,
                ),
            ));
        }
        ad_block = ad_block.child(self.edit_value(
            panel,
            GeneralField::AdException,
            "Add site exception",
            String::new(),
            cx,
        ));

        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(section("Search engine").child(search).child(self.toggle(
                "search-suggestions",
                "Autocomplete searches and URLs",
                settings.search_suggestions_enabled(),
                |settings, enabled| settings.search_suggest_enabled = Some(enabled),
                cx,
            )))
            .child(appearance)
            .child(tabs)
            .child(ad_block)
            .child(
                section("Downloads")
                    .child(self.edit_value(
                        panel,
                        GeneralField::DownloadFolder,
                        "Download folder",
                        settings.downloads_folder.clone().unwrap_or_default(),
                        cx,
                    ))
                    .child(self.toggle(
                        "ask-download",
                        "Ask where to save each download",
                        settings.ask_where_to_save.unwrap_or(false),
                        |settings, enabled| settings.ask_where_to_save = Some(enabled),
                        cx,
                    )),
            )
            .child(self.environments(panel, settings, cx))
            .child(self.permissions(settings, cx))
    }

    fn environments(&self, panel: &SettingsPanel, settings: &Settings, cx: &Context<Self>) -> Div {
        let mut content = section("Developer environments");
        for project in &settings.environments {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(self.edit_value(
                        panel,
                        GeneralField::Environment(Some(project.id.clone())),
                        &project.name,
                        format!("{}\n{}", project.name, format_targets(&project.targets)),
                        cx,
                    ))
                    .child(
                        row()
                            .child(muted(format!("{} targets", project.targets.len())))
                            .child(self.preference(
                                format!("remove-environment-{}", project.id),
                                "Remove project",
                                false,
                                {
                                    let id = project.id.clone();
                                    move |settings| {
                                        settings.environments.retain(|project| project.id != id)
                                    }
                                },
                                cx,
                            )),
                    ),
            );
        }
        content.child(self.edit_value(
            panel,
            GeneralField::Environment(None),
            "Add project",
            String::new(),
            cx,
        ))
    }

    fn permissions(&self, settings: &Settings, cx: &Context<Self>) -> Div {
        let mut content = section("Site permissions");
        let Some(sites) = settings
            .site_permissions
            .as_ref()
            .filter(|sites| !sites.is_empty())
        else {
            return content.child(muted("No saved site permissions."));
        };
        for (site, rules) in sites {
            for (kind, decision) in rules {
                let enable = *decision == PermissionDecision::Block;
                let edit = self.preference(
                    format!("permission-{site}-{kind}"),
                    if enable { "Allow" } else { "Block" },
                    false,
                    {
                        let (site, kind) = (site.clone(), kind.clone());
                        move |settings| {
                            if let Some(rule) = settings
                                .site_permissions
                                .as_mut()
                                .and_then(|sites| sites.get_mut(&site))
                                .and_then(|rules| rules.get_mut(&kind))
                            {
                                *rule = if enable {
                                    PermissionDecision::Allow
                                } else {
                                    PermissionDecision::Block
                                };
                            }
                        }
                    },
                    cx,
                );
                let reset = self.preference(
                    format!("permission-reset-{site}-{kind}"),
                    "Reset",
                    false,
                    {
                        let (site, kind) = (site.clone(), kind.clone());
                        move |settings| {
                            if let Some(sites) = &mut settings.site_permissions
                                && let Some(rules) = sites.get_mut(&site)
                            {
                                rules.remove(&kind);
                                if rules.is_empty() {
                                    sites.remove(&site);
                                }
                            }
                        }
                    },
                    cx,
                );
                let decision = if enable { "Blocked" } else { "Allowed" };
                content = content.child(
                    row()
                        .child(div().child(format!("{site} · {kind} · {decision}")))
                        .child(edit)
                        .child(reset),
                );
            }
        }
        content
    }

    fn preference(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        selected: bool,
        edit: impl Fn(&mut Settings) + 'static,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        button(id, label)
            .when(selected, |button| button.bg(rgb(0x27547b)))
            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                let result = this.state.borrow_mut().edit_settings(|settings| {
                    edit(settings);
                    Ok(String::new())
                });
                if let Some(panel) = this.settings_mut() {
                    panel.result(result);
                }
                cx.refresh_windows();
            }))
    }

    fn toggle(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        edit: fn(&mut Settings, bool),
        cx: &Context<Self>,
    ) -> Div {
        row()
            .justify_between()
            .child(div().child(label))
            .child(self.preference(
                id,
                if enabled { "On" } else { "Off" },
                enabled,
                move |settings| edit(settings, !enabled),
                cx,
            ))
    }

    fn edit_value(
        &self,
        panel: &SettingsPanel,
        field: GeneralField,
        label: &str,
        value: String,
        cx: &Context<Self>,
    ) -> Div {
        if let Some(Draft {
            field: Field::General(active),
            text,
        }) = &panel.field
            && *active == field
        {
            let content = row().child(div().child(label.to_owned())).child(
                button("general-save", "Save").on_click(cx.listener(
                    move |this, _: &ClickEvent, _, cx| {
                        let Some(Draft {
                            field: Field::General(field),
                            text,
                        }) = this.settings_mut().and_then(|p| p.field.as_ref())
                        else {
                            return;
                        };
                        let (field, text) = (field.clone(), text.text().to_owned());
                        this.submit_general(field, &text, cx);
                    },
                )),
            );
            return div().flex().flex_col().gap_1().child(content).child(text.render(field.placeholder()))
                .when(matches!(field, GeneralField::Environment(_)), |content| {
                    content.child(muted("First line is the project name. Each later line is an optional label followed by a web address."))
                });
        }
        let (action, value) = match &field {
            GeneralField::AdException | GeneralField::Environment(None) => {
                ("Add".into(), String::new())
            }
            GeneralField::Environment(Some(_)) => ("Edit".into(), value),
            _ if value.is_empty() => ("Default".into(), value),
            _ => (value.clone(), value),
        };
        row()
            .justify_between()
            .child(div().child(label.to_owned()))
            .child(button(field.id(), action).on_click(cx.listener(
                move |this, _: &ClickEvent, _, cx| {
                    if let Some(panel) = this.settings_mut() {
                        panel.begin(Field::General(field.clone()), value.clone());
                        cx.notify();
                    }
                },
            )))
    }

    pub(super) fn submit_general(
        &mut self,
        field: GeneralField,
        text: &str,
        cx: &mut Context<Self>,
    ) {
        let result = self.state.borrow_mut().edit_settings(|settings| {
            field.apply(settings, text)?;
            Ok(String::new())
        });
        if let Some(panel) = self.settings_mut() {
            if result.is_ok() {
                panel.field = None;
            }
            panel.result(result);
        }
        cx.refresh_windows();
    }
}

#[cfg(test)]
mod tests {
    use flecha_core::SIDEBAR_MAX_WIDTH;

    use super::*;
    use crate::Controller;

    #[test]
    fn general_edits_are_validated_and_persisted_before_the_next_launch() -> Result<()> {
        let profile = tempfile::tempdir()?;
        let mut controller = Controller::open_test_profile(profile.path())?;
        let download_dir = profile.path().join("download");
        controller.edit_settings(|settings| {
            settings.theme = Theme::Light;
            settings.search_engine = SearchEngine::Kagi;
            settings.ad_block = Some(false);
            settings.open_links_in_flechinha = Some(false);
            settings.peek_pinned_links = Some(false);
            settings.sidebar_in_page_fullscreen = Some(true);
            settings.ask_where_to_save = Some(true);
            GeneralField::Width.apply(settings, "999")?;
            GeneralField::ArchiveHours.apply(settings, "1.5")?;
            GeneralField::DownloadFolder.apply(settings, download_dir.to_str().unwrap())?;
            GeneralField::AdException.apply(settings, "Example.com")?;
            GeneralField::Environment(None).apply(settings, "Work\nLocal http://localhost:3000")
        })?;
        let saved = controller.model.core.settings.clone();
        assert_eq!(saved.sidebar_width.pixels(), f64::from(SIDEBAR_MAX_WIDTH));
        assert_eq!(saved.archive_policy.hours(), 1.5);
        assert!(!saved.blocks_ads_on("www.example.com"));
        for (field, invalid) in [
            (GeneralField::Width, "NaN"),
            (GeneralField::ArchiveHours, "-1"),
            (GeneralField::AdException, "https://example.com/"),
            (GeneralField::DownloadFolder, "relative"),
            (GeneralField::Environment(None), "Work\ninvalid@address"),
        ] {
            assert!(
                controller
                    .edit_settings(|settings| field.apply(settings, invalid))
                    .is_err()
            );
        }
        assert_eq!(controller.model.core.settings, saved);
        assert_eq!(
            Controller::open_test_profile(profile.path())?
                .model
                .core
                .settings,
            saved
        );
        Ok(())
    }
}
