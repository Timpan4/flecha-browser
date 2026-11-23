use std::io::BufRead as _;

use anyhow::{Context as _, Result, ensure};
use flecha_cef::{BrowserEvent, Event, Owner};
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
    Promote,
    Detach,
    Return,
    Close,
    Switch { space: SpaceId },
    NewSpace,
    Isolate,
    Quit,
    Snapshot,
    Recreate { tab: TabId },
    Script { tab: TabId, source: String },
}

impl Command {
    fn run(self, state: &mut Controller, window: &WindowId) -> Result<Value> {
        let action = match self {
            Self::Open { url } => Action::Open(url),
            Self::Select { tab } => Action::Select(tab),
            Self::Split { url } => Action::SplitNew(url),
            Self::Peek { url } => Action::Peek(url),
            Self::Promote => Action::Promote,
            Self::Detach => Action::Detach,
            Self::Return => Action::Return,
            Self::Close => Action::Close,
            Self::Switch { space } => Action::Switch(space),
            Self::NewSpace => Action::NewSpace,
            Self::Isolate => Action::Isolate,
            Self::Quit => Action::Quit,
            Self::Snapshot => return snapshot(state),
            Self::Recreate { tab } => {
                let owner = state.engine.owner(&tab).context("no engine owner")?.clone();
                let viewport = state.engine.viewport(&tab).context("no engine viewport")?;
                // Both sync CEF creations emit OnAfterCreated before the pump
                // dispatches. Only the second generation can reach the shell.
                for _ in [(), ()] {
                    state.engine.close(&tab);
                    state.pane(owner.clone(), tab.clone(), viewport)?;
                }
                return Ok(json!(state.engine.browser_id(&tab)));
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
                None => Value::Null,
            };
            json!({
                "tab": id,
                "owner": owner,
                "browser": state.engine.browser_id(id),
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({
        "core": state.model.core,
        "window": state.model.core.focused_window().id,
        "peeks": state.model.peeks.iter().map(|(window, page)| json!({ "window": window, "tab": page.id })).collect::<Vec<_>>(),
        "flechinhas": state.model.flechinhas.iter().map(|(window, page)| json!({ "window": window, "tab": page.id })).collect::<Vec<_>>(),
        "browsers": browsers,
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
                match request.command.run(&mut state, &id) {
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

pub(super) fn event(message: &BrowserEvent) {
    let tab = &message.key.tab;
    let value = match &message.event {
        Event::Created(browser) => json!({ "type": "created", "tab": tab, "browser": browser }),
        Event::Closed => json!({ "type": "closed", "tab": tab }),
        Event::Loaded { status, url } => {
            json!({ "type": "loaded", "tab": tab, "status": status, "url": url })
        }
        Event::Title(title) => json!({ "type": "title", "tab": tab, "title": title }),
        Event::Paint => json!({ "type": "paint", "tab": tab }),
        _ => return,
    };
    emit(value);
}
