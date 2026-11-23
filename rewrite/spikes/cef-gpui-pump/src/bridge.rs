//! In-process agent bridge: per-browser Chrome DevTools Protocol calls through
//! `CefBrowserHost::SendDevToolsMessage` and `AddDevToolsMessageObserver`.
//! No remote-debugging port or pipe is opened (`include/cef_browser.h`: these
//! calls "do not require an active DevTools front-end or remote-debugging session").

use anyhow::{Context as _, Result, anyhow, bail};
use base64::Engine as _;
use cef::*;
use futures::{FutureExt as _, channel::oneshot};
use gpui::BackgroundExecutor;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc as StdRc;
use std::time::Duration;

type Pending = StdRc<RefCell<HashMap<i32, oneshot::Sender<std::result::Result<Value, Value>>>>>;

struct Tab {
    id: i32,
    browser: Browser,
    _registration: Registration,
}

pub struct Bridge {
    next_id: Cell<i32>,
    pending: Pending,
    events: StdRc<Cell<u64>>,
    tabs: RefCell<Vec<Tab>>,
    executor: BackgroundExecutor,
    timeout: Duration,
}

wrap_dev_tools_message_observer! {
    struct Observer {
        pending: Pending,
        events: StdRc<Cell<u64>>,
    }

    impl DevToolsMessageObserver {
        fn on_dev_tools_message(
            &self,
            _browser: Option<&mut Browser>,
            message: Option<&[u8]>,
        ) -> ::std::os::raw::c_int {
            let Some(message) = message else { return 0 };
            let Ok(value) = serde_json::from_slice::<Value>(message) else { return 0 };
            match value.get("id").and_then(Value::as_i64) {
                Some(id) => {
                    let waiter = self.pending.borrow_mut().remove(&(id as i32));
                    if let Some(waiter) = waiter {
                        let outcome = match value.get("error") {
                            Some(err) => Err(err.clone()),
                            None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = waiter.send(outcome);
                    }
                }
                None => self.events.set(self.events.get() + 1),
            }
            1
        }

        fn on_dev_tools_agent_detached(&self, browser: Option<&mut Browser>) {
            let id = browser.map(|b| b.identifier());
            eprintln!("[bridge] DevTools agent detached from browser {id:?}");
        }
    }
}

impl Bridge {
    /// `timeout` bounds each call: CEF drops malformed messages without a reply.
    pub fn new(executor: BackgroundExecutor, timeout: Duration) -> Self {
        Self {
            next_id: Cell::new(1),
            pending: Default::default(),
            events: Default::default(),
            tabs: Default::default(),
            executor,
            timeout,
        }
    }

    pub fn attach(&self, browser: &Browser) -> Result<i32> {
        let host = browser.host().context("browser has no host")?;
        let mut observer = Observer::new(self.pending.clone(), self.events.clone());
        let registration = host
            .add_dev_tools_message_observer(Some(&mut observer))
            .context("AddDevToolsMessageObserver returned null")?;
        let id = browser.identifier();
        self.tabs.borrow_mut().push(Tab {
            id,
            browser: browser.clone(),
            _registration: registration,
        });
        Ok(id)
    }

    /// Drops every observer registration and browser reference.
    pub fn detach_all(&self) {
        self.tabs.borrow_mut().clear();
        self.pending.borrow_mut().clear();
    }

    pub fn tab_ids(&self) -> Vec<i32> {
        self.tabs.borrow().iter().map(|t| t.id).collect()
    }

    pub fn events_received(&self) -> u64 {
        self.events.get()
    }

    pub async fn call(&self, tab: i32, method: &str, params: Value) -> Result<Value> {
        let host = self
            .tabs
            .borrow()
            .iter()
            .find(|t| t.id == tab)
            .and_then(|t| t.browser.host())
            .with_context(|| format!("no tab {tab}"))?;
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let (tx, rx) = oneshot::channel();
        self.pending.borrow_mut().insert(id, tx);
        let message = json!({ "id": id, "method": method, "params": params }).to_string();
        // No RefCell borrow is held across this CEF call.
        if host.send_dev_tools_message(Some(message.as_bytes())) == 0 {
            self.pending.borrow_mut().remove(&id);
            bail!("SendDevToolsMessage rejected {method}");
        }
        let mut timer = self.executor.timer(self.timeout).fuse();
        let mut rx = rx.fuse();
        futures::select_biased! {
            outcome = rx => match outcome {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(error)) => Err(anyhow!("{method} failed: {error}")),
                Err(_) => Err(anyhow!("{method}: observer dropped")),
            },
            _ = timer => {
                self.pending.borrow_mut().remove(&id);
                Err(anyhow!("{method}: no reply within {:?}", self.timeout))
            }
        }
    }

    /// One entry per attached browser: the host's id and URL plus
    /// `Target.getTargetInfo` from that browser's own DevTools session.
    pub async fn list_tabs(&self) -> Vec<Value> {
        let mut out = Vec::new();
        let tabs: Vec<(i32, String)> = self
            .tabs
            .borrow()
            .iter()
            .map(|t| {
                let url = t
                    .browser
                    .main_frame()
                    .map(|f| CefString::from(&f.url()).to_string())
                    .unwrap_or_default();
                (t.id, url)
            })
            .collect();
        for (id, url) in tabs {
            let info = self.call(id, "Target.getTargetInfo", json!({})).await;
            out.push(json!({
                "tab": id,
                "host_url": url,
                "target_info": info.as_ref().ok().and_then(|v| v.get("targetInfo")).cloned(),
                "error": info.err().map(|e| e.to_string()),
            }));
        }
        out
    }

    /// `Runtime.evaluate` in the page's main world, returning the value.
    pub async fn evaluate(&self, tab: i32, expression: &str) -> Result<Value> {
        let result = self
            .call(
                tab,
                "Runtime.evaluate",
                json!({ "expression": expression, "returnByValue": true, "awaitPromise": true }),
            )
            .await?;
        if let Some(exception) = result.get("exceptionDetails") {
            bail!("evaluate threw: {exception}");
        }
        Ok(result
            .pointer("/result/value")
            .cloned()
            .unwrap_or(Value::Null))
    }

    /// `Page.captureScreenshot` as PNG bytes.
    pub async fn screenshot(&self, tab: i32) -> Result<Vec<u8>> {
        let result = self
            .call(tab, "Page.captureScreenshot", json!({ "format": "png" }))
            .await?;
        let data = result
            .get("data")
            .and_then(Value::as_str)
            .context("no data")?;
        Ok(base64::engine::general_purpose::STANDARD.decode(data)?)
    }
}
