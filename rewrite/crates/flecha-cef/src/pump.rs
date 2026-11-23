use std::future::poll_fn;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use async_channel::{Receiver, Sender};
use async_io::Timer;
use futures::FutureExt as _;
use gpui::AsyncApp;

// CEF's external pump uses a 10 ms work slice. Chromium can omit delayed
// notifications during a call, so retain the Spike 2 33 ms periodic wake.
const PERIODIC_WAKE: Duration = Duration::from_millis(33);

struct Timers(Timer);

impl Timers {
    fn arm(&mut self, deadline: Instant) -> &mut Timer {
        // Reset this registration in place. GPUI's Linux executor leaves
        // a dropped timer in its shared queue until the original deadline.
        self.0.set_at(deadline);
        &mut self.0
    }
}

#[derive(Clone)]
pub(crate) struct Schedule {
    latest: Arc<Mutex<Option<Instant>>>,
    wake: Sender<()>,
}

pub(crate) struct Pump {
    requests: Receiver<()>,
    latest: Arc<Mutex<Option<Instant>>>,
}

impl Schedule {
    pub fn new() -> (Self, Pump) {
        // One pending wake is sufficient because CEF replaces the previous deadline.
        let (wake, requests) = async_channel::bounded(1);
        let latest = Arc::new(Mutex::new(None));
        (
            Self {
                latest: latest.clone(),
                wake,
            },
            Pump { requests, latest },
        )
    }

    pub fn request(&self, delay_ms: i64) {
        let delay = Duration::from_millis(u64::try_from(delay_ms).unwrap_or(0));
        *self.latest.lock().expect("CEF scheduling lock") = Instant::now().checked_add(delay);
        let _ = self.wake.try_send(());
    }
}

impl Pump {
    /// Runs on CEF's UI thread. Only this task can call the pump, and a ready
    /// request wakes it through the channel without arming another timer.
    pub async fn run(self, cx: &mut AsyncApp, mut after_work: impl FnMut(&mut gpui::App) -> bool) {
        let mut timers = Timers(Timer::never());
        let mut last_start = Instant::now();
        let mut periodic = last_start;
        loop {
            let requested = *self.latest.lock().expect("CEF scheduling lock");
            let due = next_deadline(requested, last_start, periodic);
            let now = Instant::now();
            if now < due {
                let timer = timers.arm(due).fuse();
                let request = self.requests.recv().fuse();
                futures::pin_mut!(timer, request);
                futures::select_biased! {
                    _ = timer => {}
                    result = request => if result.is_err() { return; },
                }
                continue;
            }
            // Consume notifications served by this call, including a future
            // deadline if the periodic wake had to run sooner.
            let start = {
                let mut latest = self.latest.lock().expect("CEF scheduling lock");
                let start = Instant::now();
                if next_deadline(*latest, last_start, periodic) > start {
                    continue;
                }
                latest.take();
                start
            };
            last_start = start;
            cef::do_message_loop_work();
            periodic = Instant::now() + PERIODIC_WAKE;
            if !cx.update(&mut after_work) {
                return;
            }
            let mut yielded = false;
            poll_fn(|cx| {
                if std::mem::replace(&mut yielded, true) {
                    Poll::Ready(())
                } else {
                    cx.waker().wake_by_ref();
                    Poll::Pending
                }
            })
            .await;
        }
    }
}

fn next_deadline(requested: Option<Instant>, last_start: Instant, periodic: Instant) -> Instant {
    match requested {
        Some(at) if at >= last_start => at.min(periodic),
        _ => periodic,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn earliest_unserved_deadline_or_periodic_wake() {
        let start = Instant::now();
        let periodic = start + PERIODIC_WAKE;
        assert_eq!(next_deadline(Some(start), start, periodic), start);
        assert_eq!(
            next_deadline(Some(start - PERIODIC_WAKE), start, periodic),
            periodic
        );
        assert_eq!(
            next_deadline(Some(periodic + PERIODIC_WAKE), start, periodic),
            periodic
        );
    }
}
