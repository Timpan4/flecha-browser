//! Spike 2: GPUI drives CEF's external message pump and reaches its page
//! through per-browser DevTools calls. See REPORT.md.
//!
//! The spike runs on Linux only: it reads `/proc`, waits on pidfds and drives
//! CEF on X11. Other targets build a stub so the workspace still checks there.

#[cfg(target_os = "linux")]
mod app;
#[cfg(target_os = "linux")]
mod bridge;
#[cfg(target_os = "linux")]
mod config;
#[cfg(target_os = "linux")]
mod engine;
#[cfg(target_os = "linux")]
mod probe;
#[cfg(target_os = "linux")]
mod pump;
#[cfg(target_os = "linux")]
mod scenario;
#[cfg(target_os = "linux")]
mod timers;
#[cfg(target_os = "linux")]
mod view;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    app::main()
}

#[cfg(not(target_os = "linux"))]
fn main() -> std::process::ExitCode {
    eprintln!("cef-gpui-pump runs on Linux only");
    std::process::ExitCode::FAILURE
}
