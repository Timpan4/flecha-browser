use std::path::Path;
use std::rc::Rc;

use flecha_cef::{DialogKind, DialogRequest, DialogResponse, FileMode};
use flecha_permissions::Origin;
use gpui::{
    ClickEvent, Context, Div, FontWeight, InteractiveElement as _, Keystroke, ParentElement as _,
    PathPromptOptions, StatefulInteractiveElement as _, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, relative,
};

use crate::text_field::TextField;
use crate::theme::palette;
use crate::tokens;
use crate::view::{Shell, overlay_panel, primary_button, text_button};

pub(crate) struct BrowserDialog {
    request: DialogRequest,
    input: Option<TextField>,
    password: TextField,
    password_focused: bool,
}

impl BrowserDialog {
    fn new(request: DialogRequest) -> Self {
        let input = match &request.kind {
            DialogKind::Prompt { default, .. } => Some(TextField::new(default.clone(), true)),
            DialogKind::Auth { .. } => Some(TextField::default()),
            _ => None,
        };
        Self {
            request,
            input,
            password: TextField::password(),
            password_focused: false,
        }
    }

    fn accept(self) -> DialogResponse {
        let text = self.input.as_ref().map_or("", TextField::text).to_owned();
        match self.request.kind {
            DialogKind::Auth { .. } => DialogResponse::Credentials {
                username: text,
                password: self.password.text().to_owned(),
            },
            DialogKind::File { .. } => DialogResponse::Cancel,
            _ => DialogResponse::Accept(text),
        }
    }
}

impl Shell {
    pub(crate) fn sync_browser_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let pending = self.active_browser().and_then(|browser| {
            self.state
                .borrow()
                .engine
                .pending_dialog(&browser.tab)
                .map(|request| (browser.tab, request))
        });
        let Some((tab, request)) = pending else {
            if self.browser_dialog.take().is_some()
                && !self.overlay.is_open()
                && self.active_browser().is_some()
            {
                self.page_focus.focus(window, cx);
            }
            return;
        };
        if self
            .browser_dialog
            .as_ref()
            .is_none_or(|dialog| dialog.request.id != request.id)
        {
            self.browser_dialog = Some(BrowserDialog::new(request.clone()));
            self.focus.focus(window, cx);
        }
        if let DialogKind::File { mode, default_path } = request.kind
            && window.is_window_active()
            && self
                .state
                .borrow()
                .engine
                .claim_file_dialog(&tab, &request.id)
        {
            self.pick_browser_paths(request.id, mode, &default_path, cx);
        }
    }

    fn pick_browser_paths(
        &self,
        id: flecha_cef::DialogId,
        mode: FileMode,
        default_path: &Path,
        cx: &mut Context<Self>,
    ) {
        let state = Rc::downgrade(&self.state);
        let (open, save) = match mode {
            FileMode::Save => {
                let (directory, name) = if default_path.is_dir() {
                    (Some(default_path), None)
                } else {
                    (
                        default_path
                            .parent()
                            .filter(|dir| dir.is_absolute() && dir.is_dir()),
                        default_path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .filter(|name| !name.contains('\0')),
                    )
                };
                let fallback = directory.is_none().then(std::env::home_dir).flatten();
                let directory = directory
                    .or(fallback.as_deref())
                    .unwrap_or_else(|| Path::new("/"));
                (None, Some(cx.prompt_for_new_path(directory, name)))
            }
            _ => (
                Some(cx.prompt_for_paths(PathPromptOptions {
                    files: !matches!(mode, FileMode::Folder),
                    directories: matches!(mode, FileMode::Folder),
                    multiple: matches!(mode, FileMode::OpenMultiple),
                    prompt: None,
                })),
                None,
            ),
        };
        cx.spawn(async move |_, cx| {
            let result = if let Some(paths) = open {
                paths
                    .await
                    .map(|result| result.map(|paths| paths.unwrap_or_default()))
            } else if let Some(path) = save {
                path.await
                    .map(|result| result.map(|path| path.into_iter().collect()))
            } else {
                return;
            };
            if let Some(state) = state.upgrade() {
                let response = match result {
                    Ok(Ok(paths)) => DialogResponse::Paths(paths),
                    _ => DialogResponse::Cancel,
                };
                cx.update(|cx| {
                    state.borrow().engine.resolve_dialog(&id, response);
                    cx.refresh_windows();
                });
            }
        })
        .detach();
    }

    pub(crate) fn browser_dialog_key(
        &mut self,
        stroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = &mut self.browser_dialog else {
            return;
        };
        let auth = matches!(dialog.request.kind, DialogKind::Auth { .. });
        match stroke.key.as_str() {
            "escape" => self.finish_browser_dialog(false, window, cx),
            "tab" if auth => {
                dialog.password_focused = !dialog.password_focused;
                cx.notify();
            }
            "enter" if auth && !dialog.password_focused => {
                dialog.password_focused = true;
                cx.notify();
            }
            "enter" if !matches!(dialog.request.kind, DialogKind::File { .. }) => {
                self.finish_browser_dialog(true, window, cx);
            }
            _ => {
                let field = if dialog.password_focused {
                    Some(&mut dialog.password)
                } else {
                    dialog.input.as_mut()
                };
                if let Some(field) = field {
                    field.key(stroke, cx);
                    cx.notify();
                }
            }
        }
    }

    fn finish_browser_dialog(&mut self, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = self.browser_dialog.take() else {
            return;
        };
        let id = dialog.request.id.clone();
        let response = if accept {
            dialog.accept()
        } else {
            DialogResponse::Cancel
        };
        self.state.borrow().engine.resolve_dialog(&id, response);
        if !self.overlay.is_open() {
            self.page_focus.focus(window, cx);
        }
        cx.refresh_windows();
    }

    pub(crate) fn browser_dialog_panel(&self, cx: &Context<Self>) -> Option<Div> {
        let dialog = self.browser_dialog.as_ref()?;
        let page = |url: &str| {
            Origin::parse(url).map_or_else(|_| "This page".to_owned(), |origin| origin.to_string())
        };
        let (title, message, accept, cancel) = match &dialog.request.kind {
            DialogKind::Alert { origin, message } => {
                (page(origin), message.clone(), Some("OK"), None)
            }
            DialogKind::Confirm { origin, message }
            | DialogKind::Prompt {
                origin, message, ..
            } => (page(origin), message.clone(), Some("OK"), Some("Cancel")),
            DialogKind::BeforeUnload { reload } => (
                if *reload {
                    "Reload page?"
                } else {
                    "Leave page?"
                }
                .to_owned(),
                "Changes you made may be lost.".to_owned(),
                Some(if *reload { "Reload" } else { "Leave" }),
                Some("Stay"),
            ),
            DialogKind::Auth {
                host,
                port,
                realm,
                proxy,
                https,
            } => {
                let target = if host.contains(':') && !host.starts_with('[') {
                    format!("[{host}]:{port}")
                } else {
                    format!("{host}:{port}")
                };
                let mut message = if realm.is_empty() {
                    target
                } else {
                    format!("{target}\n{realm}")
                };
                if !https {
                    message.push_str(if *proxy {
                        "\nEncryption for this proxy could not be verified."
                    } else {
                        "\nThis connection does not encrypt your password."
                    });
                }
                (
                    if *proxy {
                        "Sign in to proxy"
                    } else {
                        "Sign in to site"
                    }
                    .to_owned(),
                    message,
                    Some("Sign in"),
                    Some("Cancel"),
                )
            }
            DialogKind::File { .. } => (
                "File picker".to_owned(),
                "Choose a path in the system dialog.".to_owned(),
                None,
                Some("Cancel"),
            ),
        };
        let auth = matches!(dialog.request.kind, DialogKind::Auth { .. });
        let field = |field: &TextField, label, secret| {
            div()
                .id(if secret {
                    "dialog-password"
                } else {
                    "dialog-input"
                })
                .aria_label(label)
                .when(auth, |field| {
                    field.child(div().text_size(px(tokens::TEXT_SM)).child(label))
                })
                .child(
                    field
                        .render(if auth { "" } else { label })
                        .when(auth, |field| {
                            field
                                .border_1()
                                .border_color(if dialog.password_focused == secret {
                                    palette().focus
                                } else {
                                    palette().surface_line
                                })
                        }),
                )
                .when(auth, |field| {
                    field.on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        if let Some(dialog) = &mut this.browser_dialog {
                            dialog.password_focused = secret;
                            cx.notify();
                        }
                    }))
                })
        };
        Some(
            div()
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(palette().backdrop)
                .child(
                    overlay_panel()
                        .w(px(tokens::PALETTE_WIDTH))
                        .max_w(relative(0.9))
                        .max_h(relative(0.9))
                        .p(px(tokens::SPACE_4))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(title))
                        .child(
                            div()
                                .id("dialog-message")
                                .min_h_0()
                                .overflow_y_scroll()
                                .text_color(palette().text_2)
                                .child(message),
                        )
                        .children(dialog.input.as_ref().map(|input| {
                            field(input, if auth { "Username" } else { "Response" }, false)
                        }))
                        .when(auth, |panel| {
                            panel.child(field(&dialog.password, "Password", true))
                        })
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(tokens::SPACE_2))
                                .children(cancel.map(|label| {
                                    text_button("dialog-cancel", label).on_click(cx.listener(
                                        |this, _: &ClickEvent, window, cx| {
                                            this.finish_browser_dialog(false, window, cx)
                                        },
                                    ))
                                }))
                                .children(accept.map(|label| {
                                    primary_button("dialog-accept", label).on_click(cx.listener(
                                        |this, _: &ClickEvent, window, cx| {
                                            this.finish_browser_dialog(true, window, cx)
                                        },
                                    ))
                                })),
                        ),
                ),
        )
    }
}
