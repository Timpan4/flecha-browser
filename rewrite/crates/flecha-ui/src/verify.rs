use std::io::{self, BufRead as _};
use std::num::NonZeroU16;

use anyhow::{Context as _, Result, ensure};
use flecha_adblock::{
    DOWNLOAD_TIMEOUT, Download, DownloadRequest, LISTS, UpdateReport, UreqDownload, WireResponse,
    fetch::ListWarning,
};
use flecha_cef::{BrowserEvent, Disposition, Event, Owner, Partition};
use flecha_core::{FolderId, SpaceId, TabId, WindowId};
use futures::StreamExt as _;
use gpui::App;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::sidebar::SidebarAction;
use crate::{Action, Controller, Shared};

#[derive(Deserialize)]
struct Request {
    seq: u64,
    window: Option<WindowId>,
    #[serde(flatten)]
    command: Command,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum Command {
    Open {
        url: String,
    },
    Select {
        tab: TabId,
    },
    Split {
        url: String,
    },
    Peek {
        url: String,
    },
    ClosePeek,
    Promote,
    Detach,
    Return,
    Close,
    CloseWindow,
    OpenWindow {
        url: String,
    },
    NewIncognitoWindow,
    /// Site Controls' "Allow" for location on the window's current page.
    AllowGeolocation,
    Switch {
        space: SpaceId,
    },
    NewSpace,
    Isolate,
    Quit,
    Snapshot,
    Capture,
    Discard {
        tab: TabId,
    },
    Recreate {
        tab: TabId,
    },
    Script {
        tab: TabId,
        source: String,
    },
    /// Resizes the window's content area, in logical pixels.
    Resize {
        width: f32,
        height: f32,
    },
    /// A left click at page coordinates, sent through CEF for displays without an
    /// input injection protocol.
    Click {
        tab: TabId,
        x: f32,
        y: f32,
        #[serde(default)]
        right: bool,
    },
    /// Writes `text`, when given, to GPUI's clipboard, then reads the clipboard's text back.
    Clipboard {
        text: Option<String>,
    },
    /// Dispatches a keystroke such as `ctrl-v` to the window, for displays without an input
    /// injection protocol. It replies once the window has handled the keystroke.
    Key {
        keystroke: String,
    },
    /// Chooses the enabled row labelled `label` in the open tab or Space menu, or else the page
    /// menu, and replies once it has.
    MenuRow {
        label: String,
    },
    /// Makes the shell handle page clipboard edits as it does on Wayland, to check them on a
    /// display whose compositor gives GPUI no input serial.
    ShellClipboard,
    /// Makes the next shared-texture import fail, to check the switch to CPU copies.
    RejectNextSharedTexture,
    /// Saves the appearance setting, as Settings' System, Light and Dark choices do.
    Theme {
        theme: flecha_core::Theme,
    },
    /// The sidebar's "+ Folder" with this name typed in.
    NewFolder {
        name: String,
    },
    /// Pins the tab in the active Space, inside the folder when one is named.
    Pin {
        tab: TabId,
        folder: Option<FolderId>,
    },
    Favorite {
        tab: TabId,
    },
}

impl Command {
    fn run(
        self,
        state: &mut Controller,
        window: &WindowId,
        cx: &mut App,
        seq: u64,
    ) -> Result<Value> {
        let action = match self {
            Self::Open { url } => Action::Open(url),
            Self::Select { tab } => Action::Select(tab),
            Self::Split { url } => Action::SplitNew(url),
            Self::Peek { url } => Action::Peek(url),
            Self::ClosePeek => Action::ClosePeek,
            Self::Promote => Action::Promote,
            Self::Detach => Action::Detach,
            Self::Return => Action::Return,
            Self::Close => Action::Close,
            Self::CloseWindow => Action::CloseWindow,
            Self::Switch { space } => Action::Switch(space),
            Self::NewSpace => Action::NewSpace,
            Self::Isolate => Action::Isolate,
            Self::NewFolder { name } => Action::Sidebar(SidebarAction::FolderName(None, name)),
            Self::Pin { tab, folder } => Action::Sidebar(SidebarAction::Pin(tab, folder)),
            Self::Favorite { tab } => Action::Sidebar(SidebarAction::Favorite(tab)),
            Self::Quit => Action::Quit,
            Self::NewIncognitoWindow => Action::NewIncognitoWindow,
            Self::Snapshot => return snapshot(state),
            // The model call a page's NEW_WINDOW link makes, without a Shift-click.
            Self::OpenWindow { url } => {
                let window = state.model.open_window_page(
                    crate::model::tab_id(),
                    url,
                    &Partition::Shared,
                )?;
                state.save()?;
                return Ok(json!({ "window": window }));
            }
            Self::Capture => {
                let handle = *state
                    .windows
                    .get(window)
                    .context("no GPUI window to capture")?;
                handle.update(cx, |_, window, _| {
                    // GPUI runs next-frame callbacks before drawing. Redraw
                    // in the first callback and acknowledge in the following one.
                    window.on_next_frame(move |window, _| {
                        window.refresh();
                        window.on_next_frame(move |_, _| {
                            emit(json!({ "type": "presented", "seq": seq }))
                        });
                    });
                    window.refresh();
                })?;
                return Ok(Value::Null);
            }
            Self::Discard { tab } => {
                ensure!(
                    matches!(state.engine.owner(&tab), Some(Owner::Tab(_)))
                        && state
                            .model
                            .core
                            .windows()
                            .iter()
                            .all(|window| !window.visible_tabs().contains(&tab)),
                    "only a hidden tab can be discarded for verification"
                );
                state
                    .engine
                    .discard_tab(&tab)
                    .context("could not discard the tab")?;
                return snapshot(state);
            }
            Self::Recreate { tab } => {
                let owner = state.engine.owner(&tab).context("no engine owner")?.clone();
                let viewport = state.engine.viewport(&tab).context("no engine viewport")?;
                // Both sync CEF creations emit OnAfterCreated before the pump
                // dispatches. Only the second generation can reach the shell.
                state.engine.close(&tab);
                state.pane(owner.clone(), tab.clone(), viewport)?;
                let retired = state
                    .engine
                    .queue_verification_callbacks(&tab)
                    .context("no generation to retire")?;
                state.engine.close(&tab);
                state.pane(owner, tab.clone(), viewport)?;
                let current = state
                    .engine
                    .key(&tab)
                    .context("no replacement generation")?;
                return Ok(json!({
                    "retired": retired.generation(),
                    "current": current.generation(),
                    "browser": state.engine.browser_id(&tab),
                }));
            }
            Self::AllowGeolocation => {
                let site = state
                    .site_for_window(window)
                    .context("no web page in this window")?;
                let kind = flecha_permissions::PermissionKind::Geolocation;
                state.act(
                    window,
                    Action::Site(
                        site.clone(),
                        crate::site_controls::SiteChange::Permission(
                            kind,
                            flecha_permissions::Decision::Allow,
                        ),
                    ),
                )?;
                let decision = state.site_permissions(&site).decision(kind);
                return Ok(json!({ "allowed": decision == flecha_permissions::Decision::Allow }));
            }
            Self::Script { tab, source } => {
                ensure!(
                    state.engine.execute_script(&tab, &source),
                    "missing CEF main frame"
                );
                return Ok(Value::Null);
            }
            Self::Resize { width, height } => {
                let handle = *state
                    .windows
                    .get(window)
                    .context("no GPUI window to resize")?;
                handle.update(cx, |_, window, _| {
                    window.resize(gpui::size(gpui::px(width), gpui::px(height)));
                })?;
                return Ok(Value::Null);
            }
            Self::Theme { theme } => {
                state.edit_settings(|settings| {
                    settings.theme = theme;
                    Ok(())
                })?;
                cx.refresh_windows();
                return Ok(Value::Null);
            }
            Self::RejectNextSharedTexture => {
                state.engine.reject_next_shared_texture();
                return Ok(Value::Null);
            }
            Self::Click { tab, x, y, right } => {
                let host = state.engine.host(&tab).context("missing CEF host")?;
                state.engine.focused(&tab, true);
                let button = if right {
                    gpui::MouseButton::Right
                } else {
                    gpui::MouseButton::Left
                };
                for up in [false, true] {
                    flecha_surface::input::click(
                        &host,
                        gpui::point(gpui::px(x), gpui::px(y)),
                        gpui::Modifiers::default(),
                        button,
                        1,
                        up,
                    );
                }
                return Ok(Value::Null);
            }
            Self::ShellClipboard => {
                state.shell_clipboard = true;
                return Ok(Value::Null);
            }
            Self::Clipboard { text } => {
                if let Some(text) = text {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
                }
                let text = cx.read_from_clipboard().and_then(|item| item.text());
                return Ok(json!({ "text": text }));
            }
            Self::Key { keystroke } => {
                let keystroke = gpui::Keystroke::parse_with_os(&keystroke, cx.platform_info().os)
                    .map_err(|_| anyhow::anyhow!("not a keystroke"))?;
                let handle = *state.windows.get(window).context("no GPUI window")?;
                // The shell borrows the controller this command holds, and dispatching renders
                // the shell, so neither may be held here. The deferred dispatch replies.
                cx.defer(move |cx| {
                    let sent =
                        gpui::AppContext::update_window(cx, handle.into(), |_, window, cx| {
                            window.dispatch_keystroke(keystroke, cx);
                        });
                    reply(seq, sent.map(|()| Value::Null));
                });
                return Ok(Value::Null);
            }
            Self::MenuRow { label } => {
                let handle = *state.windows.get(window).context("no GPUI window")?;
                // Deferred and replied to like Key. The row is looked up in the same update
                // that uses it, so it belongs to the menu that is open then.
                cx.defer(move |cx| {
                    let used = handle.update(cx, |shell, window, cx| {
                        if matches!(shell.overlay, crate::view::Overlay::Menu(_)) {
                            shell.use_menu_row(&label, window, cx)?;
                        } else {
                            let row = shell.context_menu_row(&label)?;
                            shell.use_context_menu(row, window, cx);
                        }
                        anyhow::Ok(Value::Null)
                    });
                    reply(seq, used.and_then(|used| used));
                });
                return Ok(Value::Null);
            }
        };
        state.act(window, action)?;
        snapshot(state)
    }
}

fn snapshot(state: &Controller) -> Result<Value> {
    let browsers = state
        .engine
        .ids()
        .map(|id| {
            let owner = match state.engine.owner(id) {
                Some(Owner::Tab(window)) => json!({ "kind": "tab", "window": window }),
                Some(Owner::Peek(window)) => json!({ "kind": "peek", "window": window }),
                Some(Owner::Flechinha(window)) => json!({ "kind": "flechinha", "window": window }),
                Some(Owner::Popup(window)) => json!({ "kind": "popup", "window": window }),
                None => Value::Null,
            };
            let partition = state.engine.partition(id).map_or(Value::Null, partition);
            json!({
                "tab": id,
                "owner": owner,
                "partition": partition,
                "browser": state.engine.browser_id(id),
                "loading": state.engine.is_loading(id),
                "audible": state.engine.is_audible(id),
                "muted": state.engine.is_audio_muted(id),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "core": state.model.core,
        "window": state.model.core.focused_window().id,
        "peeks": state.model.peeks.iter().map(|(window, page)| json!({ "window": window, "tab": page.id })).collect::<Vec<_>>(),
        "flechinhas": state.model.flechinhas.iter().map(|(window, page)| json!({ "window": window, "tab": page.id })).collect::<Vec<_>>(),
        "popups": state.model.popups.iter().map(|(window, popup)| json!({ "window": window, "tab": popup.page.id, "opener": popup.opener })).collect::<Vec<_>>(),
        "browsers": browsers,
        "incognito": state.model.incognito.as_ref().map(|incognito| json!({
            "session": incognito.session.number(),
            "state": incognito.state,
        })),
        "contexts": state.engine.context_partitions().map(partition).collect::<Vec<_>>(),
        "downloads": state.windows.keys().map(|window| (window.as_str().to_owned(), json!(state.download_rows(window)))).collect::<serde_json::Map<_, _>>(),
        "liveBrowsers": state.engine.live_browsers(),
        "nativeWindows": state.windows.keys().collect::<Vec<_>>(),
    }))
}

fn partition(partition: &Partition) -> Value {
    match partition {
        Partition::Shared => json!("shared"),
        Partition::Isolated(space) => json!(space),
        Partition::Incognito(session) => json!(format!("incognito-{}", session.number())),
    }
}

pub(super) fn install(controller: &Shared, cx: &mut App) {
    let (tx, mut rx) = futures::channel::mpsc::unbounded();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if tx.unbounded_send(line).is_err() {
                break;
            }
        }
    });
    let controller = controller.clone();
    cx.spawn(async move |cx| {
        while let Some(line) = rx.next().await {
            cx.update(|cx| {
                let request = match serde_json::from_str::<Request>(&line) {
                    Ok(request) => request,
                    Err(_) => {
                        emit(json!({ "type": "invalid_request" }));
                        return;
                    }
                };
                let mut state = controller.borrow_mut();
                let id = request
                    .window
                    .unwrap_or_else(|| state.model.core.focused_window().id.clone());
                let deferred = matches!(
                    request.command,
                    Command::Key { .. } | Command::MenuRow { .. }
                );
                match request.command.run(&mut state, &id, cx, request.seq) {
                    Ok(_) if deferred => {}
                    result => reply(request.seq, result),
                }
                cx.refresh_windows();
            });
        }
    })
    .detach();
}

fn reply(seq: u64, result: Result<Value>) {
    emit(match result {
        Ok(result) => json!({ "type": "reply", "seq": seq, "result": result }),
        Err(error) => json!({ "type": "reply", "seq": seq, "error": error.to_string() }),
    });
}

fn emit(value: Value) {
    println!("phase1 {value}");
}

struct FixtureDownload {
    port: u16,
    inner: UreqDownload,
}

impl Download for FixtureDownload {
    fn get(&self, url: &str, request: &DownloadRequest) -> io::Result<WireResponse> {
        let list = LISTS
            .iter()
            .find(|list| list.url == url)
            .ok_or_else(|| io::Error::other("unexpected filter list URL"))?;
        self.inner.get(
            &format!("http://127.0.0.1:{}/{}.txt", self.port, list.id),
            request,
        )
    }
}

pub(super) fn adblock_download(port: NonZeroU16) -> Box<dyn Download> {
    Box::new(FixtureDownload {
        port: port.get(),
        inner: UreqDownload::default(),
    })
}

pub(super) fn adblock_report(report: &UpdateReport) {
    emit(json!({
        "type": "adblock",
        "failed": !report.failures.is_empty(),
        "usedCache": report.warnings.iter().any(|warning| matches!(warning, ListWarning::UsedCache { .. })),
        "downloadTimeoutMs": DOWNLOAD_TIMEOUT.as_millis(),
    }));
}

pub(super) fn ready() {
    emit(json!({ "type": "ready" }));
}

/// The shell asked the platform to bring this window to the front.
pub(super) fn activated(window: &WindowId) {
    emit(json!({ "type": "activated", "window": window }));
}

/// A page's Copy or Cut finished reading its selection. The text itself is never reported.
pub(super) fn page_copy(tab: &TabId, copied: bool) {
    emit(json!({ "type": "page_copy", "tab": tab, "copied": copied }));
}

/// Every CEF browser has closed and the app is about to quit.
pub(super) fn drained(live: usize) {
    emit(json!({ "type": "drained", "live": live }));
}

#[cfg(target_os = "linux")]
pub(super) fn print_ready(tab: &TabId, file: &std::fs::File) {
    use std::os::unix::fs::FileExt as _;

    let mut header = [0; b"%PDF-".len()];
    let pdf = file.read_exact_at(&mut header, 0).is_ok() && header == *b"%PDF-";
    emit(json!({ "type": "print_ready", "tab": tab, "pdf": pdf }));
}

pub(super) fn event(message: &BrowserEvent) {
    let tab = &message.key.tab;
    let mut value = match &message.event {
        Event::Created(browser) => json!({ "type": "created", "tab": tab, "browser": browser }),
        Event::Closed => json!({ "type": "closed", "tab": tab }),
        Event::Loaded { status, url } => {
            json!({ "type": "loaded", "tab": tab, "status": status, "url": url })
        }
        Event::LoadFailed { url, code } => {
            json!({ "type": "load_failed", "tab": tab, "url": url, "code": code })
        }
        Event::Address(address) => json!({ "type": "address", "tab": tab, "address": address }),
        Event::Title(title) => json!({ "type": "title", "tab": tab, "title": title }),
        Event::Paint => json!({ "type": "paint", "tab": tab }),
        Event::Opened(opened) => json!({
            "type": "opened",
            "tab": tab,
            "url": opened.url,
            "disposition": match opened.disposition {
                Disposition::ForegroundTab => "foreground_tab",
                Disposition::BackgroundTab => "background_tab",
                Disposition::Window => "window",
                Disposition::Popup => "popup",
            },
            "size": opened.size,
        }),
        Event::ClosedByPage => json!({ "type": "closed_by_page", "tab": tab }),
        Event::Fullscreen(fullscreen) => {
            json!({ "type": "fullscreen", "tab": tab, "fullscreen": fullscreen })
        }
        Event::FaviconUrls(urls) => json!({ "type": "favicon_urls", "tab": tab, "urls": urls }),
        Event::Loading(loading) => json!({ "type": "loading", "tab": tab, "loading": loading }),
        Event::Audible(audible) => json!({ "type": "audible", "tab": tab, "audible": audible }),
        Event::DialogChanged => json!({ "type": "dialog_changed", "tab": tab }),
        Event::SecurityChanged => json!({ "type": "security_changed", "tab": tab }),
        Event::PaintFailed | Event::PointerChanged => return,
    };
    value["generation"] = json!(message.key.generation());
    emit(value);
}
