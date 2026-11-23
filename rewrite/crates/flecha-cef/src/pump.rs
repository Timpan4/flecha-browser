use std::future::poll_fn;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::{Duration, Instant};

use futures::channel::mpsc;
use futures::{FutureExt as _, StreamExt as _};
use gpui::{AsyncApp, BackgroundExecutor, Task};

// CEF's external pump uses a 10 ms work slice. Chromium can omit delayed
// notifications during a call, so retain the Spike 2 33 ms periodic wake.
const PERIODIC_WAKE: Duration = Duration::from_millis(33);

pub(crate) struct Timers(BackgroundExecutor);

impl Timers {
    #[allow(clippy::disallowed_methods)]
    fn arm(&self, after: Duration) -> Task<()> {
        self.0.timer(after)
    }
}

#[derive(Clone)]
pub(crate) struct Schedule {
    latest: Arc<Mutex<Option<Instant>>>,
    wake: mpsc::Sender<()>,
}

pub(crate) struct Pump {
    requests: mpsc::Receiver<()>,
    latest: Arc<Mutex<Option<Instant>>>,
}

impl Schedule {
    pub fn new() -> (Self, Pump) {
        // One pending wake is sufficient because CEF replaces the previous deadline.
        let (wake, requests) = mpsc::channel(0);
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
        let now = Instant::now();
        let delay = Duration::from_millis(u64::try_from(delay_ms).unwrap_or(0));
        *self.latest.lock().expect("CEF scheduling lock") = now.checked_add(delay);
        let _ = self.wake.clone().try_send(());
    }
}

impl Pump {
    /// Runs on CEF's UI thread. Only this task can call the pump, and a ready
    /// request wakes it through the channel without arming another timer.
    pub async fn run(
        mut self,
        cx: &mut AsyncApp,
        mut after_work: impl FnMut(&mut gpui::App) -> bool,
    ) {
        let timers = Timers(cx.background_executor().clone());
        let mut last_start = Instant::now();
        let mut periodic = last_start;
        let mut armed = None;
        loop {
            let requested = *self.latest.lock().expect("CEF scheduling lock");
            let due = next_deadline(requested, last_start, periodic);
            let now = Instant::now();
            if now < due {
                if armed.as_ref().is_none_or(|(deadline, _)| *deadline > due) {
                    armed = Some((due, timers.arm(due - now).fuse()));
                }
                let (deadline, mut timer) = armed.take().expect("armed deadline");
                futures::select_biased! {
                    _ = timer => {}
                    request = self.requests.next() => {
                        if request.is_none() { return; }
                        armed = Some((deadline, timer));
                    }
                }
                continue;
            }
            // Consume notifications served by this call, including a future
            // deadline if the periodic wake had to run sooner.
            last_start = {
                let mut latest = self.latest.lock().expect("CEF scheduling lock");
                latest.take();
                Instant::now()
            };
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
