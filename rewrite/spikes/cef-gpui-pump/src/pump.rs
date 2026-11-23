//! Drives CEF's external message pump from GPUI's main thread. The timing
//! rules and the reentrancy guard live in `cef_gpui_pump::schedule`, which has
//! unit tests; this module wires them to CEF and GPUI.
//!
//! Two CEF behaviors shape the trace (see REPORT.md, "Pump and idle"):
//! - One `CefDoMessageLoopWork()` can take up to 10 ms. CEF 8c29be2
//!   `libcef/browser/browser_message_loop.cc` builds `MessagePumpExternal(0.01f)`,
//!   whose `Run()` keeps running work until the next task is more than
//!   `max_time_slice_` (10 ms) past the call's start. That time blocks GPUI's
//!   main thread.
//! - CEF rarely asks with `delay_ms > 0`. Chromium's
//!   `ThreadControllerWithMessagePumpImpl::SetNextDelayedDoWork` only schedules
//!   delayed work when the thread is not inside `DoWork`, so CEF's timers rely
//!   on the periodic call.

use cef_gpui_pump::schedule::{Event, Guard, Reason, Scheduler};
pub use cef_gpui_pump::schedule::{MAX_PUMP_INTERVAL, analyze};
use futures::{FutureExt as _, StreamExt as _, channel::mpsc};
use gpui::AsyncApp;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

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

thread_local! {
    static GUARD: Guard = Guard::default();
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
    let mut c = COUNTS.with_borrow(|c| *c);
    c.recursive_calls_deferred = GUARD.with(Guard::deferred);
    c
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

/// One guarded `CefDoMessageLoopWork()` call.
fn do_work(reason: Reason) {
    let at = Instant::now();
    let ran = GUARD.with(|g| g.run(cef::do_message_loop_work));
    if !ran {
        return;
    }
    let took = at.elapsed();
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
    let mut scheduler = Scheduler::new(Instant::now());
    loop {
        let (due, reason) = scheduler.next();
        let mut timer = executor
            .timer(due.saturating_duration_since(Instant::now()))
            .fuse();
        futures::select_biased! {
            _ = timer => {}
            delay = requests.next() => {
                let Some(delay_ms) = delay else { return };
                let now = Instant::now();
                record(Event::Request { at: now, delay_ms });
                COUNTS.with_borrow_mut(|c| if delay_ms <= 0 {
                    c.requests_immediate += 1
                } else {
                    c.requests_delayed += 1
                });
                scheduler.request(now, delay_ms);
                continue;
            }
        }
        do_work(reason);
        let rerun = GUARD.with(Guard::take_rerun);
        scheduler.called(Instant::now(), rerun);
    }
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
