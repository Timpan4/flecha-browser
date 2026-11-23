use std::io::BufRead as _;

use anyhow::{Context as _, Result, ensure};
use flecha_cef::{BrowserEvent, Disposition, Event, Owner, Partition};
use flecha_core::{SpaceId, TabId, WindowId};
use futures::StreamExt as _;
use gpui::App;
use serde::Deserialize;
use serde_json::{Value, json};

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
    Open { url: String },
    Select { tab: TabId },
    Split { url: String },
    Peek { url: String },
    ClosePeek,
    Promote,
    Detach,
    Return,
    Close,
    CloseWindow,
    OpenWindow { url: String },
    Switch { space: SpaceId },
    NewSpace,
    Isolate,
    Quit,
    Snapshot,
    Capture,
    Recreate { tab: TabId },
    Script { tab: TabId, source: String },
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
            Self::Quit => Action::Quit,
            Self::Snapshot => return snapshot(state),
            // The model call a page's NEW_WINDOW link makes, without a Shift-click.
            Self::OpenWindow { url } => {
                let window = state.model.open_window_page(crate::model::tab_id(), url)?;
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
            Self::Script { tab, source } => {
                ensure!(
                    state.engine.execute_script(&tab, &source),
                    "missing CEF main frame"
                );
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
            let partition = match state.engine.partition(id) {
                Some(Partition::Shared) => json!("shared"),
                Some(Partition::Isolated(space)) => json!(space),
                None => Value::Null,
            };
            json!({
                "tab": id,
                "owner": owner,
                "partition": partition,
                "browser": state.engine.browser_id(id),
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
        "liveBrowsers": state.engine.live_browsers(),
        "nativeWindows": state.windows.keys().collect::<Vec<_>>(),
    }))
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
                match request.command.run(&mut state, &id, cx, request.seq) {
                    Ok(result) => {
                        emit(json!({ "type": "reply", "seq": request.seq, "result": result }))
                    }
                    Err(error) => emit(
                        json!({ "type": "reply", "seq": request.seq, "error": error.to_string() }),
                    ),
                }
                cx.refresh_windows();
            });
        }
    })
    .detach();
}

fn emit(value: Value) {
    println!("phase1 {value}");
}

pub(super) fn ready() {
    emit(json!({ "type": "ready" }));
}

/// The shell asked the platform to bring this window to the front.
pub(super) fn activated(window: &WindowId) {
    emit(json!({ "type": "activated", "window": window }));
}

/// Every CEF browser has closed and the app is about to quit.
pub(super) fn drained(live: usize) {
    emit(json!({ "type": "drained", "live": live }));
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
        Event::PaintFailed => return,
    };
    value["generation"] = json!(message.key.generation());
    emit(value);
}
