use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{Context as _, Result, bail, ensure};
use flecha_cef::Runtime;
use flecha_services::StateStore;
use flecha_ui::Options;

fn main() -> Result<()> {
    let profile = isolated_profile()?;
    let runtime = Rc::new(RefCell::new(Runtime::entry()?));
    let mut options = Options {
        initial: None,
        verify: false,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--open" => options.initial = Some(args.next().context("--open requires an address")?),
            "--verify" => options.verify = true,
            _ => bail!("unknown Flecha option"),
        }
    }
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
            if let Err(error) = runtime.initialize(&cef_profile).and_then(|()| {
                flecha_ui::start(
                    cx,
                    &mut runtime,
                    cef_profile,
                    store,
                    loaded,
                    options,
                    app_failure.clone(),
                )
            }) {
                *app_failure.borrow_mut() = Some(error);
                cx.quit();
            }
        });
    drop(runtime);
    failure.take().map_or(Ok(()), Err)
}

fn isolated_profile() -> Result<PathBuf> {
    const SCRATCH: &str = "/mnt/scratch";
    ensure!(
        std::env::var("FLECHA_LAUNCHER").as_deref() == Ok("isolated")
            && std::env::var_os("HOME").as_deref()
                == Some(Path::new(SCRATCH).join("home").as_os_str())
            && std::env::var_os("XDG_RUNTIME_DIR").as_deref()
                == Some(Path::new(SCRATCH).join("runtime").as_os_str()),
        "Flecha must be started by rewrite/tools/isolated-run.sh"
    );
    let profile = Path::new(SCRATCH).join("profile");
    ensure!(
        std::env::var_os("FLECHA_PROFILE").as_deref() == Some(profile.as_os_str()),
        "the launcher must supply a scratch profile"
    );
    Ok(profile)
}
