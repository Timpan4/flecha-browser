//! Drives CEF's external message pump from GPUI's main thread.
//!
//! Contract (CEF 158.0.3, `include/cef_app.h` `CefDoMessageLoopWork` and
//! `include/cef_browser_process_handler.h` `OnScheduleMessagePumpWork`):
//! - `OnScheduleMessagePumpWork(delay_ms)` arrives on any thread. `delay_ms <= 0`
//!   means "soon"; `delay_ms > 0` replaces any pending call with one after
//!   `min(delay_ms, max interval)`.
//! - The app must also call `CefDoMessageLoopWork()` when no request arrives.
//! - `CefDoMessageLoopWork()` runs only on the main thread and must not recurse.

use futures::{FutureExt as _, StreamExt as _, channel::mpsc};
use gpui::AsyncApp;
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// `cef_app.h`: "A maximum interval of approximately 33 milliseconds between
/// calls (30 calls per second) is recommended."
pub const MAX_PUMP_INTERVAL: Duration = Duration::from_millis(33);

static SCHEDULE_TX: OnceLock<mpsc::UnboundedSender<i64>> = OnceLock::new();
static REQUESTS_RECEIVED: AtomicU64 = AtomicU64::new(0);

/// Called from CEF's `OnScheduleMessagePumpWork` on any thread. Only marshals.
pub fn request_from_cef(delay_ms: i64) {
    REQUESTS_RECEIVED.fetch_add(1, Ordering::Relaxed);
    if let Some(tx) = SCHEDULE_TX.get() {
        let _ = tx.unbounded_send(delay_ms);
    }
}

/// Must run before `CefInitialize`, which can already request work.
pub fn install() -> mpsc::UnboundedReceiver<i64> {
    let (tx, rx) = mpsc::unbounded();
    SCHEDULE_TX.set(tx).expect("pump installed twice");
    rx
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// CEF asked with `delay_ms <= 0`.
    Immediate,
    /// CEF asked with `delay_ms > 0` and that delay elapsed.
    Delayed,
    /// No request arrived within `MAX_PUMP_INTERVAL`, or a delayed request was
    /// longer than it and was capped.
    Periodic,
    /// A call was requested while one was running; it runs after the active call.
    Rerun,
}

#[derive(Clone, Copy, Debug)]
pub enum Event {
    Request {
        at: Instant,
        delay_ms: i64,
    },
    Call {
        at: Instant,
        reason: Reason,
        took: Duration,
    },
}

thread_local! {
    static IN_WORK: Cell<bool> = const { Cell::new(false) };
    static RERUN: Cell<bool> = const { Cell::new(false) };
    static TRACE: RefCell<Option<Vec<Event>>> = const { RefCell::new(None) };
    static COUNTS: RefCell<Counts> = RefCell::new(Counts::default());
}

#[derive(Clone, Copy, Default, Debug)]
pub struct Counts {
    pub requests_immediate: u64,
    pub requests_delayed: u64,
    pub calls_immediate: u64,
    pub calls_delayed: u64,
    pub calls_periodic: u64,
    pub calls_rerun: u64,
    pub recursive_calls_deferred: u64,
}

impl Counts {
    pub fn calls(&self) -> u64 {
        self.calls_immediate + self.calls_delayed + self.calls_periodic + self.calls_rerun
    }
}

pub fn counts() -> Counts {
    COUNTS.with_borrow(|c| *c)
}

pub fn requests_received_any_thread() -> u64 {
    REQUESTS_RECEIVED.load(Ordering::Relaxed)
}

/// Start recording every request and call until `stop_trace`.
pub fn start_trace() {
    TRACE.with_borrow_mut(|t| *t = Some(Vec::new()));
}

pub fn stop_trace() -> Vec<Event> {
    TRACE.with_borrow_mut(|t| t.take()).unwrap_or_default()
}

fn record(event: Event) {
    TRACE.with_borrow_mut(|t| {
        if let Some(t) = t {
            t.push(event)
        }
    });
}

/// One guarded `CefDoMessageLoopWork()` call. A call made while another is
/// active is deferred until the active one returns, as `cef_app.h` requires.
fn do_work(reason: Reason) {
    if IN_WORK.get() {
        RERUN.set(true);
        COUNTS.with_borrow_mut(|c| c.recursive_calls_deferred += 1);
        return;
    }
    IN_WORK.set(true);
    let at = Instant::now();
    cef::do_message_loop_work();
    let took = at.elapsed();
    IN_WORK.set(false);
    COUNTS.with_borrow_mut(|c| match reason {
        Reason::Immediate => c.calls_immediate += 1,
        Reason::Delayed => c.calls_delayed += 1,
        Reason::Periodic => c.calls_periodic += 1,
        Reason::Rerun => c.calls_rerun += 1,
    });
    record(Event::Call { at, reason, took });
}

/// The pump loop. Runs on GPUI's main thread until the request channel closes
/// or the app quits.
pub async fn run(mut requests: mpsc::UnboundedReceiver<i64>, cx: &mut AsyncApp) {
    let executor = cx.background_executor().clone();
    let mut due = Instant::now() + MAX_PUMP_INTERVAL;
    let mut reason = Reason::Periodic;
    loop {
        let wait = due.saturating_duration_since(Instant::now());
        let mut timer = executor.timer(wait).fuse();
        futures::select_biased! {
            _ = timer => {}
            delay = requests.next() => {
                let Some(delay_ms) = delay else { return };
                let now = Instant::now();
                record(Event::Request { at: now, delay_ms });
                if delay_ms <= 0 {
                    COUNTS.with_borrow_mut(|c| c.requests_immediate += 1);
                    due = now;
                    reason = Reason::Immediate;
                } else {
                    COUNTS.with_borrow_mut(|c| c.requests_delayed += 1);
                    let asked = Duration::from_millis(delay_ms as u64);
                    if asked <= MAX_PUMP_INTERVAL {
                        due = now + asked;
                        reason = Reason::Delayed;
                    } else {
                        due = now + MAX_PUMP_INTERVAL;
                        reason = Reason::Periodic;
                    }
                }
                continue;
            }
        }
        do_work(reason);
        if RERUN.replace(false) {
            due = Instant::now();
            reason = Reason::Rerun;
        } else {
            due = Instant::now() + MAX_PUMP_INTERVAL;
            reason = Reason::Periodic;
        }
    }
}

/// Checks a trace against the contract: every call is either answering a CEF
/// request, a rerun, or a periodic call made only after `MAX_PUMP_INTERVAL`
/// passed with no call and no pending request.
pub fn analyze(trace: &[Event]) -> Value {
    let mut last_call: Option<Instant> = None;
    let mut last_request: Option<(Instant, i64)> = None;
    let mut gaps_ms: Vec<f64> = Vec::new();
    let mut work_ms: Vec<f64> = Vec::new();
    let mut unexplained = Vec::new();
    let mut early_periodic = 0u64;
    let mut calls = 0u64;
    let mut requests = 0u64;
    let first = trace.first().map(|e| match e {
        Event::Request { at, .. } | Event::Call { at, .. } => *at,
    });
    for event in trace {
        match *event {
            Event::Request { at, delay_ms } => {
                requests += 1;
                last_request = Some((at, delay_ms));
            }
            Event::Call { at, reason, took } => {
                calls += 1;
                work_ms.push(took.as_secs_f64() * 1e3);
                if let Some(prev) = last_call {
                    gaps_ms.push((at - prev).as_secs_f64() * 1e3);
                }
                let since_call = last_call.map(|p| at - p);
                // The most recent request strictly after the previous call.
                let pending = last_request.filter(|(r, _)| last_call.is_none_or(|c| *r >= c));
                let explained = match reason {
                    Reason::Immediate => pending.is_some_and(|(_, d)| d <= 0),
                    Reason::Delayed => pending
                        .is_some_and(|(r, d)| d > 0 && at >= r + Duration::from_millis(d as u64)),
                    Reason::Rerun => true,
                    Reason::Periodic => {
                        let quiet_since = match pending {
                            Some((r, _)) => r,
                            None => last_call.unwrap_or(at - MAX_PUMP_INTERVAL),
                        };
                        let ok = at >= quiet_since + MAX_PUMP_INTERVAL;
                        if !ok && since_call.is_some() {
                            early_periodic += 1;
                        }
                        ok || since_call.is_none()
                    }
                };
                if !explained {
                    let offset = first.map(|f| (at - f).as_secs_f64() * 1e3);
                    unexplained.push(json!({ "at_ms": offset, "reason": format!("{reason:?}") }));
                }
                last_call = Some(at);
            }
        }
    }
    gaps_ms.sort_by(f64::total_cmp);
    work_ms.sort_by(f64::total_cmp);
    let pct = |v: &[f64], q: f64| -> Option<f64> {
        (!v.is_empty()).then(|| v[((v.len() - 1) as f64 * q).round() as usize])
    };
    json!({
        "requests": requests,
        "calls": calls,
        "unexplained_calls": unexplained.len(),
        "unexplained_examples": unexplained.iter().take(10).collect::<Vec<_>>(),
        "early_periodic_calls": early_periodic,
        "gap_between_calls_ms": {
            "min": pct(&gaps_ms, 0.0), "p50": pct(&gaps_ms, 0.5),
            "p99": pct(&gaps_ms, 0.99), "max": pct(&gaps_ms, 1.0),
        },
        "work_duration_ms": {
            "p50": pct(&work_ms, 0.5), "p99": pct(&work_ms, 0.99), "max": pct(&work_ms, 1.0),
            "total": work_ms.iter().sum::<f64>(),
            "slowest": work_ms.iter().rev().take(5).collect::<Vec<_>>(),
        },
    })
}

pub fn counts_json(c: Counts) -> Value {
    json!({
        "requests_immediate": c.requests_immediate,
        "requests_delayed": c.requests_delayed,
        "calls_immediate": c.calls_immediate,
        "calls_delayed": c.calls_delayed,
        "calls_periodic": c.calls_periodic,
        "calls_rerun": c.calls_rerun,
        "calls_total": c.calls(),
        "recursive_calls_deferred": c.recursive_calls_deferred,
    })
}

pub fn counts_delta(a: Counts, b: Counts) -> Counts {
    Counts {
        requests_immediate: b.requests_immediate - a.requests_immediate,
        requests_delayed: b.requests_delayed - a.requests_delayed,
        calls_immediate: b.calls_immediate - a.calls_immediate,
        calls_delayed: b.calls_delayed - a.calls_delayed,
        calls_periodic: b.calls_periodic - a.calls_periodic,
        calls_rerun: b.calls_rerun - a.calls_rerun,
        recursive_calls_deferred: b.recursive_calls_deferred - a.recursive_calls_deferred,
    }
}
