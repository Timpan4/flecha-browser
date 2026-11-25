use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result, ensure};
use async_compat::Compat;
use flecha_cef::extensions::{
    MANAGED_POLICY_SUPPORTED, STORE_UPDATE_URL, crx_manifest, requested_permissions, store_id,
};
use gpui::{
    ClickEvent, Context, Div, IntoElement as _, ParentElement as _, PathPromptOptions,
    StatefulInteractiveElement as _, Styled as _, Task, div, prelude::FluentBuilder as _, px,
};

use super::{Draft, Field, SettingsPanel, SettingsTab, card, row, section};
use crate::theme::palette;
use crate::tokens;
use crate::view::{Shell, danger_button, primary_button, text_button};

#[derive(Default)]
pub(super) struct Editor {
    review: Option<Review>,
    pending: Option<Task<()>>,
    picker: Option<Task<()>>,
}

impl Editor {
    /// Drops the review and any download for a Store input that changed.
    pub(super) fn invalidate_review(&mut self) {
        self.review = None;
        self.pending = None;
    }
}

struct Review {
    id: String,
    name: String,
    version: String,
    permissions: Vec<String>,
}

async fn review(id: String) -> Result<Review> {
    use cef::sys::{
        CHROME_VERSION_BUILD, CHROME_VERSION_MAJOR, CHROME_VERSION_MINOR, CHROME_VERSION_PATCH,
    };
    // Chrome Web Store accepts packages up to 2 GB (developer.chrome.com/docs/webstore/publish).
    const STORE_PACKAGE_BYTES: u64 = 2_000_000_000;
    let version = format!(
        "{CHROME_VERSION_MAJOR}.{CHROME_VERSION_MINOR}.{CHROME_VERSION_BUILD}.{CHROME_VERSION_PATCH}"
    );
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    };
    let (name, version, permissions) = Compat::new(async {
        let redirects = reqwest::redirect::Policy::default();
        let client = reqwest::Client::builder()
            .https_only(true)
            .referer(false)
            .timeout(flecha_adblock::DOWNLOAD_TIMEOUT)
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if matches!(
                    attempt.url().host_str(),
                    Some("clients2.google.com" | "clients2.googleusercontent.com")
                ) {
                    redirects.redirect(attempt)
                } else {
                    attempt.error("Unexpected extension package host")
                }
            }))
            .build()?;
        let mut url = reqwest::Url::parse(STORE_UPDATE_URL)?;
        url.query_pairs_mut().extend_pairs([
            ("os", "linux"),
            ("arch", arch),
            ("prod", "chromiumcrx"),
            ("prodversion", &version),
            ("acceptformat", "crx3"),
            ("response", "redirect"),
            ("x", &format!("id={id}&installsource=ondemand&uc")),
        ]);
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?;
        let mut crx = tempfile::NamedTempFile::new()?;
        let mut downloaded = 0;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(reqwest::Error::without_url)?
        {
            downloaded += u64::try_from(chunk.len())?;
            ensure!(
                downloaded <= STORE_PACKAGE_BYTES,
                "Store extension exceeds the package size limit"
            );
            crx.write_all(&chunk)?;
        }
        let manifest = crx_manifest(crx.path())?;
        let name = manifest
            .get("name")
            .and_then(|value| value.as_str())
            .context("Missing extension name")?;
        let version = manifest
            .get("version")
            .and_then(|value| value.as_str())
            .context("Missing extension version")?;
        let name = if name.starts_with("__MSG_") {
            id.clone()
        } else {
            name.to_owned()
        };
        Ok::<_, anyhow::Error>((name, version.to_owned(), requested_permissions(&manifest)?))
    })
    .await
    .context("Read Chrome Web Store permissions")?;
    Ok(Review {
        id,
        name,
        version,
        permissions,
    })
}

impl Shell {
    pub(super) fn extension_settings(&self, panel: &SettingsPanel, cx: &Context<Self>) -> Div {
        let state = self.state.borrow();
        let settings = &state.extensions;
        let installed = self.extension_entries(&state);
        let mut list = card();
        if let Some(problem) = &settings.problem {
            list = list.child(row().child(note(format!(
                "Extension settings could not be read, so the lists below may be empty or out of date: {problem}"
            ))));
        }
        if let Some(skipped) = &settings.skipped {
            list = list.child(row().child(note(format!(
                "This launch skipped every unpacked folder: {skipped}"
            ))));
        }
        if installed.is_empty() {
            list = list.child(row().child("No extensions installed in this Space."));
        }
        for extension in installed {
            let capability = if extension.popup.is_some() {
                "Popup available. Actions that need the active tab may fail."
            } else if extension.has_action {
                "This action has no popup. Stock CEF cannot run its click handler."
            } else {
                "No toolbar action."
            };
            list = list.child(
                row()
                    .items_center()
                    .gap(px(tokens::SPACE_2))
                    .child(self.extension_icon(&state, extension))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .child(extension.name.clone())
                            .child(note(format!("{} · {capability}", extension.version)))
                            .children(extension.popup.as_ref().map(|url| note(url.clone()))),
                    )
                    .when(settings.managed.contains(&extension.id), |row| {
                        row.child(self.remove_managed_button(extension.id.clone(), cx))
                    }),
            );
        }
        for id in settings
            .managed
            .iter()
            .filter(|id| !installed.iter().any(|extension| extension.id == **id))
        {
            list = list.child(
                row()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .min_w_0()
                            .child(format!("{id} (not installed in this Space)")),
                    )
                    .child(self.remove_managed_button(id.clone(), cx)),
            );
        }
        let mut store = card();
        if MANAGED_POLICY_SUPPORTED {
            let input = match &panel.field {
                Some(Draft {
                    field: Field::StoreExtension,
                    text,
                }) => text.render("Chrome Web Store URL or ID").into_any_element(),
                _ => text_button("store-extension-input", "Chrome Web Store URL or ID")
                    .relative()
                    .child(self.track_extension_control("store-input".into(), Default::default()))
                    .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                        if let Some(panel) = shell.settings_mut() {
                            panel.begin(Field::StoreExtension, String::new());
                            cx.notify();
                        }
                    }))
                    .into_any_element(),
            };
            store = store.child(
                row()
                    .items_center()
                    .gap(px(tokens::SPACE_2))
                    .child(input)
                    .child(
                        primary_button("review-extension", "Review permissions").on_click(
                            cx.listener(|shell, _: &ClickEvent, _, cx| {
                                shell.review_store_extension(cx);
                            }),
                        ),
                    ),
            );
            if panel.extensions.pending.is_some() {
                store = store.child(row().child(note("Reading permissions…")));
            }
            if let Some(review) = &panel.extensions.review {
                let permissions = div().flex().flex_col().gap(px(tokens::SPACE_1)).min_w_0()
                    .child(format!("{} · {}", review.name, review.version))
                    .child(note("This is the Store's current version. Flecha adds the extension ID to its managed list, and on restart Chromium installs whatever version the Store serves then, which can be newer than this one. Managed installation grants that version's required permissions automatically, and later updates can change them."))
                    .child(note(if review.permissions.is_empty() { "No permissions declared.".into() } else { review.permissions.join(", ") }));
                store = store.child(
                    row()
                        .flex_col()
                        .gap(px(tokens::SPACE_2))
                        .child(permissions)
                        .child(
                            primary_button("install-reviewed-extension", "Add to managed installs")
                                .relative()
                                .child(self.track_extension_control(
                                    "store-confirm".into(),
                                    Default::default(),
                                ))
                                .on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                                    shell.install_reviewed_extension(cx)
                                })),
                        ),
                );
            }
        } else {
            store =
                store.child(row().child(note("Managed Store installation is available on Linux.")));
        }
        let mut unpacked = card().child(row().flex_col().gap(px(tokens::SPACE_2))
            .child(note("Unpacked extensions load when Flecha starts. Restart Flecha after changing this list."))
            .child(primary_button("choose-extension-folder", "Load unpacked folder").on_click(cx.listener(|shell, _: &ClickEvent, _, cx| {
                shell.pick_extension_folder(cx);
            }))));
        for folder in settings.folders.union(settings.starting_folders()) {
            let starts = settings.starting_folders().contains(folder);
            let enabled = settings.folders.contains(folder);
            let path = folder.clone();
            let label = match (starts, enabled) {
                (true, true) => "Configured at startup",
                (false, true) => "Loads on restart",
                _ => "Removed on restart",
            };
            unpacked = unpacked.child(
                row()
                    .items_center()
                    .justify_between()
                    .gap(px(tokens::SPACE_2))
                    .child(
                        div()
                            .min_w_0()
                            .child(folder.display().to_string())
                            .child(note(label)),
                    )
                    .when(enabled, |row| {
                        row.child(
                            danger_button(format!("forget-{}", path.display()), "Remove").on_click(
                                cx.listener(move |shell, _: &ClickEvent, _, cx| {
                                    shell.change_extensions(
                                        |settings| settings.remove_unpacked(&path),
                                        "Restart Flecha to unload the extension.",
                                        cx,
                                    );
                                }),
                            ),
                        )
                    }),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(tokens::SETTINGS_CARD_GAP))
            .child(section("Installed extensions").child(list))
            .child(section("Chrome Web Store").child(store))
            .child(section("Developer mode").child(unpacked))
    }

    pub(super) fn review_store_extension(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.settings_mut() else {
            return;
        };
        if !MANAGED_POLICY_SUPPORTED || panel.extensions.pending.is_some() {
            return;
        }
        let id = panel
            .field
            .as_ref()
            .filter(|field| field.field == Field::StoreExtension)
            .context("Enter a Chrome Web Store URL or ID")
            .and_then(|field| store_id(field.text.text()));
        panel.extensions.review = None;
        let id = match id {
            Ok(id) => id,
            Err(error) => {
                panel.result(Err(error));
                cx.notify();
                return;
            }
        };
        panel.message = None;
        let work = cx.background_executor().spawn(review(id));
        panel.extensions.pending = Some(cx.spawn(async move |shell, cx| {
            let result = work.await;
            let _ = shell.update(cx, |this, cx| {
                let verify = this.state.borrow().verify;
                let Some(panel) = this
                    .settings_mut()
                    .filter(|panel| panel.tab == SettingsTab::Extensions)
                else {
                    return;
                };
                panel.extensions.pending = None;
                match result {
                    Ok(review) => {
                        if verify {
                            crate::verify::extension_review(
                                &review.id,
                                &review.name,
                                &review.version,
                                &review.permissions,
                            );
                        }
                        panel.extensions.review = Some(review);
                    }
                    Err(error) => panel.result(Err(error)),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn install_reviewed_extension(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.settings_mut() else {
            return;
        };
        let Some(review) = panel.extensions.review.take() else {
            return;
        };
        self.change_extensions(
            |settings| settings.add_store(&review.id),
            "Added to the managed list. Restart Flecha to install.",
            cx,
        );
    }

    fn remove_managed_button(&self, id: String, cx: &Context<Self>) -> gpui::Stateful<Div> {
        let control = format!("remove-{id}");
        danger_button(control.clone(), "Remove")
            .relative()
            .child(self.track_extension_control(control, Default::default()))
            .on_click(cx.listener(move |shell, _: &ClickEvent, _, cx| {
                shell.change_extensions(
                    |settings| settings.remove_managed(&id),
                    "Removed from the managed list. Restart Flecha to apply.",
                    cx,
                );
            }))
    }

    fn change_extensions(
        &mut self,
        edit: impl FnOnce(&flecha_cef::extensions::ExtensionSettings) -> Result<()>,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        let result = {
            let mut state = self.state.borrow_mut();
            edit(&state.extensions.settings).and_then(|()| {
                state
                    .extensions
                    .reload_settings()
                    .context("Saved. Restart Flecha to apply it, but the list could not refresh")
            })
        };
        if let Some(panel) = self.settings_mut() {
            panel.result(result.map(|()| message.to_owned()));
        }
        cx.refresh_windows();
    }

    fn pick_extension_folder(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.settings_mut() else {
            return;
        };
        if panel.extensions.picker.is_some() {
            return;
        }
        let result = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose an unpacked extension folder".into()),
        });
        panel.extensions.picker = Some(cx.spawn(async move |shell, cx| {
            let selected: Result<Option<PathBuf>> = result
                .await
                .context("Extension folder picker closed")
                .and_then(|result| result)
                .map(|paths| paths.and_then(|paths| paths.into_iter().next()));
            let _ = shell.update(cx, |this, cx| {
                let Some(panel) = this
                    .settings_mut()
                    .filter(|panel| panel.tab == SettingsTab::Extensions)
                else {
                    return;
                };
                panel.extensions.picker = None;
                match selected {
                    Ok(Some(directory)) => this.change_extensions(
                        |settings| settings.add_unpacked(&directory),
                        "Restart Flecha to load the extension.",
                        cx,
                    ),
                    Ok(None) => {}
                    Err(error) => {
                        panel.result(Err(error));
                        cx.notify();
                    }
                }
            });
        }));
    }
}

fn note(text: impl Into<gpui::SharedString>) -> Div {
    div()
        .text_size(px(tokens::TEXT_SM))
        .text_color(palette().text_2)
        .child(text.into())
}
