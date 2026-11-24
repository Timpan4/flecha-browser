use std::cell::RefCell;
use std::rc::Rc;

use anyhow::{Result, bail};
use flecha_cef::{Runtime, Start};
use flecha_services::StateStore;

mod launch;

use launch::{Environment, Launch, USAGE};

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let (profile, options) = match Launch::resolve(&args, &Environment::read())? {
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
    let runtime = Rc::new(RefCell::new(Runtime::entry()?));
    let (store, loaded) = StateStore::open(&profile.join("session"))?;
    let cef_profile = profile.join("cef");
    std::fs::create_dir_all(&cef_profile)?;
    let failure = Rc::new(RefCell::new(None));
    let app_failure = failure.clone();
    let app_runtime = runtime.clone();
    gpui_platform::application()
        .with_quit_mode(gpui::QuitMode::Explicit)
        .run(move |cx| {
            let mut runtime = app_runtime.borrow_mut();
            let started = runtime.initialize(&cef_profile, options.initial.as_deref());
            if let Err(error) = started.and_then(|start| match start {
                Start::Forwarded => std::process::exit(0),
                Start::Running => flecha_ui::start(
                    cx,
                    &mut runtime,
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
