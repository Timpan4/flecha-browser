//! The only place this crate arms GPUI timers. GPUI's Linux timers all run on
//! one shared `Timer` thread and stay there until they fire, even when dropped
//! (`gpui/src/platform_scheduler.rs`, `gpui_linux/src/linux/dispatcher.rs`).
//! One timer per request piles up there and delays every timer in the process,
//! including the CEF pump's. `clippy.toml` disallows GPUI's timer methods
//! everywhere else, so every timer goes through a ledger that counts it.

use gpui::{BackgroundExecutor, Task};
use std::cell::{Cell, RefCell};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::time::{Duration, Instant};

/// Counts the timers one component arms. A timer counts as outstanding until
/// its deadline, dropped or not.
pub struct Timers {
    executor: BackgroundExecutor,
    deadlines: RefCell<BinaryHeap<Reverse<Instant>>>,
    created: Cell<u64>,
    max_outstanding: Cell<usize>,
}

impl Timers {
    pub fn new(executor: BackgroundExecutor) -> Self {
        Self {
            executor,
            deadlines: Default::default(),
            created: Cell::new(0),
            max_outstanding: Cell::new(0),
        }
    }

    pub fn arm(&self, after: Duration) -> Task<()> {
        let now = Instant::now();
        // Allowed only here, where the ledger counts it: every GPUI timer sits
        // on the one shared Linux Timer thread until its deadline.
        #[allow(clippy::disallowed_methods)]
        let timer = self.executor.timer(after);
        let mut deadlines = self.deadlines.borrow_mut();
        while deadlines.peek().is_some_and(|Reverse(at)| *at <= now) {
            deadlines.pop();
        }
        deadlines.push(Reverse(now + after));
        self.max_outstanding
            .set(self.max_outstanding.get().max(deadlines.len()));
        self.created.set(self.created.get() + 1);
        timer
    }

    pub fn created(&self) -> u64 {
        self.created.get()
    }

    pub fn max_outstanding(&self) -> usize {
        self.max_outstanding.get()
    }
}
