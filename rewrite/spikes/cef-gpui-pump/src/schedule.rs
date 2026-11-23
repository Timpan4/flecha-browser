//! When to call `CefDoMessageLoopWork()`, and the reentrancy guard around it.
//!
//! Contract (CEF 158.0.3, `include/cef_app.h` `CefDoMessageLoopWork` and
//! `include/cef_browser_process_handler.h` `OnScheduleMessagePumpWork`):
//! - `OnScheduleMessagePumpWork(delay_ms)`: `delay_ms <= 0` means "soon";
//!   `delay_ms > 0` cancels any pending call and schedules one after the delay.
//! - The app must also call `CefDoMessageLoopWork()` at most about 33 ms apart.
//! - `CefDoMessageLoopWork()` must not be called recursively.

use serde_json::{Value, json};
use std::cell::Cell;
use std::time::{Duration, Instant};

/// `cef_app.h`: "A maximum interval of approximately 33 milliseconds between
/// calls (30 calls per second) is recommended."
pub const MAX_PUMP_INTERVAL: Duration = Duration::from_millis(33);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// CEF asked with `delay_ms <= 0`.
    Immediate,
    /// CEF asked with `delay_ms > 0` and that delay elapsed.
    Delayed,
    /// `MAX_PUMP_INTERVAL` passed since the last call before any requested time.
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

/// Next call time. The periodic deadline counts from the end of the last call
/// and is never pushed back by requests, so a stream of delayed requests cannot
/// stretch the gap past `MAX_PUMP_INTERVAL`.
#[derive(Debug)]
pub struct Scheduler {
    last_call_end: Instant,
    requested: Option<(Instant, Reason)>,
}

impl Scheduler {
    pub fn new(now: Instant) -> Self {
        Self {
            last_call_end: now,
            requested: None,
        }
    }

    /// A CEF request. It replaces any earlier pending request, as the contract says.
    pub fn request(&mut self, now: Instant, delay_ms: i64) {
        self.requested = Some(if delay_ms <= 0 {
            (now, Reason::Immediate)
        } else {
            (now + Duration::from_millis(delay_ms as u64), Reason::Delayed)
        });
    }

    pub fn next(&self) -> (Instant, Reason) {
        let periodic = self.last_call_end + MAX_PUMP_INTERVAL;
        match self.requested {
            Some((at, reason)) if at <= periodic => (at, reason),
            _ => (periodic, Reason::Periodic),
        }
    }

    /// A call finished at `now`. Any pending request was served by it.
    pub fn called(&mut self, now: Instant, rerun: bool) {
        self.last_call_end = now;
        self.requested = rerun.then_some((now, Reason::Rerun));
    }
}

/// Defers a call made while another is active until the active one returns.
#[derive(Default, Debug)]
pub struct Guard {
    in_work: Cell<bool>,
    rerun: Cell<bool>,
    deferred: Cell<u64>,
}

impl Guard {
    /// Runs `work` unless a call is already active. Returns whether it ran.
    pub fn run(&self, work: impl FnOnce()) -> bool {
        if self.in_work.get() {
            self.rerun.set(true);
            self.deferred.set(self.deferred.get() + 1);
            return false;
        }
        self.in_work.set(true);
        work();
        self.in_work.set(false);
        true
    }

    /// True once after a call was deferred.
    pub fn take_rerun(&self) -> bool {
        self.rerun.replace(false)
    }

    pub fn deferred(&self) -> u64 {
        self.deferred.get()
    }
}

/// Checks a trace against the contract: every call answers a CEF request, is a
/// rerun, or is a periodic call made `MAX_PUMP_INTERVAL` after the previous one.
pub fn analyze(trace: &[Event]) -> Value {
    let mut last_call: Option<(Instant, Duration)> = None;
    let mut last_request: Option<(Instant, i64)> = None;
    let mut gaps_ms: Vec<f64> = Vec::new();
    let mut work_ms: Vec<f64> = Vec::new();
    let mut unexplained = Vec::new();
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
                if let Some((prev, _)) = last_call {
                    gaps_ms.push((at - prev).as_secs_f64() * 1e3);
                }
                // The most recent request since the previous call started.
                let pending =
                    last_request.filter(|(r, _)| last_call.is_none_or(|(c, _)| *r >= c));
                let explained = match reason {
                    Reason::Immediate => pending.is_some_and(|(_, d)| d <= 0),
                    Reason::Delayed => pending
                        .is_some_and(|(r, d)| d > 0 && at >= r + Duration::from_millis(d as u64)),
                    Reason::Rerun => true,
                    Reason::Periodic => {
                        last_call.is_none_or(|(c, took)| at >= c + took + MAX_PUMP_INTERVAL)
                    }
                };
                if !explained {
                    let offset = first.map(|f| (at - f).as_secs_f64() * 1e3);
                    unexplained.push(json!({ "at_ms": offset, "reason": format!("{reason:?}") }));
                }
                last_call = Some((at, took));
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

#[cfg(test)]
mod tests {
    use super::*;

    const MS: Duration = Duration::from_millis(1);

    #[test]
    fn delayed_request_within_the_interval_runs_at_its_delay() {
        let t0 = Instant::now();
        let mut s = Scheduler::new(t0);
        s.request(t0, 10);
        assert_eq!(s.next(), (t0 + 10 * MS, Reason::Delayed));
    }

    #[test]
    fn delayed_request_past_the_interval_is_capped_by_the_periodic_call() {
        let t0 = Instant::now();
        let mut s = Scheduler::new(t0);
        s.request(t0, 50);
        assert_eq!(s.next(), (t0 + MAX_PUMP_INTERVAL, Reason::Periodic));
    }

    #[test]
    fn a_new_request_replaces_the_pending_one() {
        let t0 = Instant::now();
        let mut s = Scheduler::new(t0);
        s.request(t0, 5);
        s.request(t0 + MS, 20);
        assert_eq!(s.next(), (t0 + 21 * MS, Reason::Delayed));
        s.request(t0 + 2 * MS, 0);
        assert_eq!(s.next(), (t0 + 2 * MS, Reason::Immediate));
    }

    #[test]
    fn a_stream_of_delayed_requests_cannot_postpone_the_periodic_call() {
        let t0 = Instant::now();
        let mut s = Scheduler::new(t0);
        // Each request would land past the periodic deadline if it restarted the clock.
        for i in 0..10u32 {
            s.request(t0 + i * 20 * MS, 30);
            let (at, _) = s.next();
            assert!(at <= t0 + MAX_PUMP_INTERVAL, "call at {:?}", at - t0);
        }
    }

    #[test]
    fn a_call_serves_the_pending_request() {
        let t0 = Instant::now();
        let mut s = Scheduler::new(t0);
        s.request(t0, 10);
        s.called(t0 + 10 * MS, false);
        assert_eq!(
            s.next(),
            (t0 + 10 * MS + MAX_PUMP_INTERVAL, Reason::Periodic)
        );
    }

    #[test]
    fn a_recursive_call_is_deferred_and_rerun_after_the_active_one() {
        let guard = Guard::default();
        let mut inner_ran = false;
        let outer_ran = guard.run(|| {
            inner_ran = guard.run(|| unreachable!("recursive call ran"));
        });
        assert!(outer_ran);
        assert!(!inner_ran);
        assert_eq!(guard.deferred(), 1);
        assert!(guard.take_rerun());
        assert!(!guard.take_rerun());

        let t0 = Instant::now();
        let mut s = Scheduler::new(t0);
        s.called(t0, true);
        assert_eq!(s.next(), (t0, Reason::Rerun));
        // A call after the guard was released runs.
        assert!(guard.run(|| {}));
    }

    #[test]
    fn analyze_accepts_the_scheduler_s_calls_and_flags_early_periodic_ones() {
        let t0 = Instant::now();
        let took = MS;
        let trace = [
            Event::Call { at: t0, reason: Reason::Periodic, took },
            Event::Request { at: t0 + 2 * MS, delay_ms: 10 },
            Event::Call { at: t0 + 12 * MS, reason: Reason::Delayed, took },
            Event::Request { at: t0 + 14 * MS, delay_ms: 50 },
            Event::Call { at: t0 + 13 * MS + MAX_PUMP_INTERVAL, reason: Reason::Periodic, took },
            // Too early: less than MAX_PUMP_INTERVAL after the previous call ended.
            Event::Call { at: t0 + 60 * MS, reason: Reason::Periodic, took },
        ];
        let a = analyze(&trace);
        assert_eq!(a["calls"], 4);
        assert_eq!(a["unexplained_calls"], 1);
        let at_ms = a["unexplained_examples"][0]["at_ms"].as_f64().unwrap();
        assert!((at_ms - 60.0).abs() < 1e-6, "{at_ms}");
    }
}
