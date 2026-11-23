//! NEGATIVE CONTROL, never part of the build: one use of each GPUI API that
//! `clippy.toml` disallows. `timer-guard-control.sh` adds it to a disposable
//! copy as `src/timer_mutants.rs` and requires one Clippy diagnostic per use.
#![allow(dead_code)]

use gpui::{
    Animation, AnyView, App, BackgroundExecutor, ForegroundExecutor, FutureExt as _,
    InteractiveElement as _, PlatformDispatcher, RunnableVariant,
    StatefulInteractiveElement as _, Window, div,
};
use std::time::Duration;

fn executor_timers(bg: &BackgroundExecutor, fg: &ForegroundExecutor) {
    let _a = bg.timer(Duration::ZERO);
    let _b = bg.scheduler_executor();
    let local = fg.scheduler_executor();
    let _c = local.timer(Duration::ZERO);
    let _d = std::future::ready(()).with_timeout(Duration::ZERO, bg);
}

fn delayed_dispatch(dispatcher: &dyn PlatformDispatcher, runnable: RunnableVariant) {
    dispatcher.dispatch_after(Duration::ZERO, runnable);
}

fn animations() {
    let _a = Animation::new(Duration::ZERO).with_max_fps(1.0);
    let mut b = Animation::new(Duration::ZERO);
    b.max_fps = Some(1.0);
}

fn tooltips(build: fn(&mut Window, &mut App) -> AnyView) {
    let _a = div().id("a").tooltip(build);
    let _b = div().id("b").hoverable_tooltip(build);
    let mut c = div();
    c.interactivity().tooltip(build);
    c.interactivity().hoverable_tooltip(build);
}

fn idle_sleep(cx: &App) {
    let _a = cx.prevent_idle_sleep("control");
}
