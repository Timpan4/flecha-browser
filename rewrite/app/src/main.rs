use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, bail};
use flecha_cef::{Runtime, Start};
use flecha_services::StateStore;

mod launch;

use launch::{Environment, Launch, USAGE};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (profile, mut options) = match Launch::resolve(&args, &Environment::read())? {
        Launch::Help => {
            print!("{USAGE}");
            return Ok(());
        }
        Launch::CefChild => {
            Runtime::entry()?;
            bail!("CEF did not handle the child process");
        }
        Launch::Browser { profile, options } => (profile, options),
    };
    if let Some(input) = options.initial.as_deref() {
        options.initial = Some(flecha_ui::validate_initial_address(input)?);
    }
    let mut runtime = Runtime::entry()?;
    runtime.configure_extensions(&profile)?;
    let runtime = Rc::new(RefCell::new(runtime));
    let (store, loaded) = StateStore::open(&profile.join("session"))?;
    let cef_profile = profile.join("cef");
    std::fs::create_dir_all(&cef_profile)?;
    let failure = Rc::new(RefCell::new(None));
    let app_failure = failure.clone();
    let app_runtime = runtime.clone();
    gpui_platform::application()
        .with_assets(flecha_ui::Assets)
        .with_quit_mode(gpui::QuitMode::Explicit)
        .run(move |cx| {
            let mut runtime = app_runtime.borrow_mut();
            // GPUI picks its GPU for the first window, and Chromium's GPU process, which CEF
            // starts during `initialize`, must share textures on that GPU. A launch CEF will
            // forward to the Flecha holding the profile opens no window.
            let first = if flecha_cef::profile_in_use(&cef_profile) {
                Ok(None)
            } else {
                flecha_ui::FirstWindow::open(cx).map(Some)
            };
            let started = first.and_then(|first| {
                let software = flecha_cef::Compositing::software();
                let compositing = first
                    .as_ref()
                    .map_or(&software, flecha_ui::FirstWindow::compositing);
                let start =
                    runtime.initialize(&cef_profile, options.initial.as_deref(), compositing)?;
                Ok((start, first))
            });
            if let Err(error) = started.and_then(|(start, first)| match start {
                Start::Forwarded => std::process::exit(0),
                Start::Running => flecha_ui::start(
                    cx,
                    &mut runtime,
                    first,
                    cef_profile,
                    store,
                    loaded,
                    options,
                    app_failure.clone(),
                ),
            }) {
                *app_failure.borrow_mut() = Some(error);
                cx.quit();
            }
        });
    drop(runtime);
    failure.take().map_or(Ok(()), Err)
}
