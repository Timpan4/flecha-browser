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

use crate::timers::Timers;
use cef_gpui_pump::schedule::{Event, Guard, Reason, Scheduler};
pub use cef_gpui_pump::schedule::{MAX_PUMP_INTERVAL, analyze, analyze_between};
use futures::{FutureExt as _, StreamExt as _, channel::mpsc};
use gpui::AsyncApp;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static SCHEDULE_TX: OnceLock<mpsc::UnboundedSender<(Instant, i64)>> = OnceLock::new();
static REQUESTS_RECEIVED: AtomicU64 = AtomicU64::new(0);

/// Called from CEF's `OnScheduleMessagePumpWork` on any thread. Only marshals,
/// with the time of the request.
pub fn request_from_cef(delay_ms: i64) {
    REQUESTS_RECEIVED.fetch_add(1, Ordering::Relaxed);
    if let Some(tx) = SCHEDULE_TX.get() {
        let _ = tx.unbounded_send((Instant::now(), delay_ms));
    }
}

/// Must run before `CefInitialize`, which can already request work.
pub fn install() -> mpsc::UnboundedReceiver<(Instant, i64)> {
    let (tx, rx) = mpsc::unbounded();
    SCHEDULE_TX.set(tx).expect("pump installed twice");
    rx
}

thread_local! {
    static GUARD: Guard = Guard::default();
    static TRACE: RefCell<Option<Vec<Event>>> = const { RefCell::new(None) };
    static COUNTS: RefCell<Counts> = RefCell::new(Counts::default());
    static TIMERS: RefCell<Option<Rc<Timers>>> = const { RefCell::new(None) };
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

/// GPUI timers the pump loop has armed since start, and the most outstanding at once.
pub fn timers() -> (u64, usize) {
    TIMERS.with_borrow(|t| {
        t.as_ref()
            .map_or((0, 0), |t| (t.created(), t.max_outstanding()))
    })
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

/// One guarded `CefDoMessageLoopWork()` call. Returns when it started and ended,
/// or `None` if the guard deferred it.
fn do_work(reason: Reason) -> Option<(Instant, Instant)> {
    let at = Instant::now();
    let ran = GUARD.with(|g| g.run(cef::do_message_loop_work));
    if !ran {
        return None;
    }
    let end = Instant::now();
    COUNTS.with_borrow_mut(|c| match reason {
        Reason::Immediate => c.calls_immediate += 1,
        Reason::Delayed => c.calls_delayed += 1,
        Reason::Periodic => c.calls_periodic += 1,
        Reason::Rerun => c.calls_rerun += 1,
    });
    record(Event::Call {
        at,
        reason,
        took: end - at,
    });
    Some((at, end))
}

fn take_request(scheduler: &mut Scheduler, (at, delay_ms): (Instant, i64)) {
    record(Event::Request { at, delay_ms });
    COUNTS.with_borrow_mut(|c| {
        if delay_ms <= 0 {
            c.requests_immediate += 1
        } else {
            c.requests_delayed += 1
        }
    });
    scheduler.request(at, delay_ms);
}

/// Returns `Pending` once, so other GPUI foreground tasks run between
/// back-to-back pump calls.
fn yield_now() -> impl std::future::Future<Output = ()> {
    let mut yielded = false;
    std::future::poll_fn(move |cx| {
        if yielded {
            std::task::Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            std::task::Poll::Pending
        }
    })
}

/// The pump loop. Runs on GPUI's main thread until the request channel closes
/// or the app quits.
///
/// It creates a GPUI timer only when no armed timer fires early enough, and
/// never for a call that is already due. GPUI's Linux timers all run on one
/// shared thread and stay there until they fire, even when dropped
/// (`gpui/src/platform_scheduler.rs`, `gpui_linux/src/linux/dispatcher.rs`), so
/// one timer per request would pile up under a stream of CEF requests.
pub async fn run(mut requests: mpsc::UnboundedReceiver<(Instant, i64)>, cx: &mut AsyncApp) {
    let timers = Rc::new(Timers::new(cx.background_executor().clone()));
    TIMERS.with_borrow_mut(|t| *t = Some(timers.clone()));
    let mut scheduler = Scheduler::new(Instant::now());
    let mut armed: Option<(Instant, futures::future::Fuse<gpui::Task<()>>)> = None;
    loop {
        // Every request already queued counts before the next call is chosen.
        loop {
            match requests.try_recv() {
                Ok(request) => take_request(&mut scheduler, request),
                Err(mpsc::TryRecvError::Closed) => return,
                Err(mpsc::TryRecvError::Empty) => break,
            }
        }
        let (due, reason) = scheduler.next();
        let now = Instant::now();
        if due > now {
            if armed.as_ref().is_none_or(|(at, _)| *at > due) {
                armed = Some((due, timers.arm(due - now).fuse()));
            }
            let (armed_at, mut timer) = armed.take().expect("armed above");
            futures::select_biased! {
                _ = timer => {}
                request = requests.next() => {
                    let Some(request) = request else { return };
                    take_request(&mut scheduler, request);
                    armed = Some((armed_at, timer));
                }
            }
            continue;
        }
        if let Some((start, end)) = do_work(reason) {
            let rerun = GUARD.with(Guard::take_rerun);
            scheduler.called(start, end, rerun);
        }
        yield_now().await;
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
