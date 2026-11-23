//! Spike 2: GPUI drives CEF's external message pump and reaches its page
//! through per-browser DevTools calls. See REPORT.md.

mod bridge;
mod config;
mod engine;
mod probe;
mod pump;
mod scenario;
mod view;

use anyhow::{Context as _, Result, bail};
use cef::{CefString, ImplCommandLine as _};
use futures::channel::mpsc;
use gpui::{App, AppContext as _, Bounds, WindowBounds, WindowOptions, point, px, size};
use serde_json::{Value, json};
use std::cell::{Cell, RefCell};
use std::process::ExitCode;
use std::rc::Rc;
use std::time::Instant;

fn main() -> ExitCode {
    // Leak check, run by scripts/run-checks.sh after the app exits. No CEF.
    if let Some(report) =
        std::env::args().find_map(|a| a.strip_prefix("--pump-wait-gone=").map(str::to_string))
    {
        return match wait_gone(&report) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::from(2),
            Err(err) => {
                eprintln!("error: {err:#}");
                ExitCode::FAILURE
            }
        };
    }
    env_logger::init();
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let args = cef::args::Args::new();
    let Some(cmd) = args.as_cmd_line() else {
        eprintln!("cannot parse command line");
        return ExitCode::FAILURE;
    };
    let is_browser = cmd.has_switch(Some(&CefString::from("type"))) != 1;
    let mut app = engine::build_app();

    // Child processes (renderer, GPU, utility, zygote) return here.
    let code = cef::execute_process(
        Some(args.as_main_args()),
        Some(&mut app),
        std::ptr::null_mut(),
    );
    if !is_browser {
        return ExitCode::from(code.clamp(0, 255) as u8);
    }

    match browser_main(args, app) {
        Ok(pass) if pass => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(2),
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn browser_main(args: cef::args::Args, mut app: cef::App) -> Result<bool> {
    let cfg = config::Config::from_args()?;
    let pump_requests = pump::install();
    let context_ready = engine::install_context_ready();

    let settings = cef::Settings {
        windowless_rendering_enabled: 1,
        external_message_pump: 1,
        multi_threaded_message_loop: 0,
        no_sandbox: cfg.no_sandbox as _,
        root_cache_path: CefString::from(cfg.cache_dir.to_string_lossy().as_ref()),
        ..Default::default()
    };
    if cef::initialize(
        Some(args.as_main_args()),
        Some(&settings),
        Some(&mut app),
        std::ptr::null_mut(),
    ) != 1
    {
        bail!("CefInitialize failed");
    }

    let report: scenario::Report = Default::default();
    let children = Rc::new(RefCell::new(Vec::new()));
    let gpui_started = Rc::new(Cell::new(false));
    {
        let report = report.clone();
        let children = children.clone();
        let gpui_started = gpui_started.clone();
        let cfg = cfg.clone();
        gpui_platform::application().run(move |cx: &mut App| {
            gpui_started.set(true);
            if let Err(err) = start(
                cx,
                cfg,
                pump_requests,
                context_ready,
                report.clone(),
                children,
            ) {
                report
                    .borrow_mut()
                    .insert("error".into(), json!(format!("{err:#}")));
                cx.quit();
            }
        });
    }

    // CefShutdown must follow every browser closing. The scenario records
    // `browsers_closed` false when it starts and true once every OnBeforeClose
    // arrived; with no record, no scenario ran and no browser exists.
    let browsers_closed = report.borrow().get("browsers_closed") != Some(&json!(false));
    let shutdown_ms = browsers_closed.then(|| {
        let t = Instant::now();
        cef::shutdown();
        t.elapsed().as_secs_f64() * 1e3
    });

    // Zygotes exit when this process closes their sockets, so some are still
    // alive here; `--pump-wait-gone` checks them after exit.
    let alive_now: Vec<Value> = children
        .borrow()
        .iter()
        .filter(|p| probe::alive(p.pid, p.start_time))
        .map(probe::Proc::summary)
        .collect();
    let me = probe::self_pid();
    let remaining: Vec<Value> = probe::tree(me)
        .iter()
        .filter(|p| p.pid != me)
        .map(probe::Proc::summary)
        .collect();

    let mut report = report.borrow().clone();
    report.insert(
        "step_timeout_ms".into(),
        json!(cfg.step_timeout.as_secs_f64() * 1e3),
    );
    report.insert("warmup_ms".into(), json!(cfg.warmup.as_secs_f64() * 1e3));
    report.insert("measure_ms".into(), json!(cfg.measure.as_secs_f64() * 1e3));
    report.insert("sandbox".into(), json!(!cfg.no_sandbox));
    report.insert("scenario".into(), json!(format!("{:?}", cfg.scenario)));
    report.insert("gpui_ran".into(), json!(gpui_started.get()));
    report.insert("pump_totals".into(), pump::counts_json(pump::counts()));
    report.insert(
        "pump_requests_received_any_thread".into(),
        json!(pump::requests_received_any_thread()),
    );
    report.insert(
        "max_pump_interval_ms".into(),
        json!(pump::MAX_PUMP_INTERVAL.as_millis()),
    );
    report.insert(
        "shutdown".into(),
        json!({
            "cef_shutdown_called": shutdown_ms.is_some(),
            "cef_shutdown_skipped_reason": (!browsers_closed).then_some("browsers still open"),
            "cef_shutdown_ms": shutdown_ms,
            "children_before_shutdown": children.borrow().iter().map(probe::Proc::summary).collect::<Vec<_>>(),
            "children_alive_right_after_cef_shutdown": alive_now,
            "descendants_right_after_cef_shutdown": remaining,
        }),
    );
    let pass_key = match cfg.scenario {
        config::Scenario::Idle => "idle_pass",
        config::Scenario::Animate => "animate_pass",
        config::Scenario::Cdp => "cdp_pass",
        config::Scenario::DevTools => "devtools_pass",
    };
    let pass = report.get(pass_key) == Some(&json!(true))
        && report.get("isolation_clean") == Some(&json!(true))
        && browsers_closed
        && !report.contains_key("error")
        && !report.contains_key("shutdown_error");
    report.insert("scenario_pass".into(), json!(pass));
    let text = serde_json::to_string_pretty(&Value::Object(report))?;
    std::fs::write(&cfg.out, &text).with_context(|| format!("write {}", cfg.out.display()))?;
    println!("{text}");
    Ok(pass)
}

/// Adds the after-exit leak check to a scenario report and sets `pass`.
fn wait_gone(path: &str) -> Result<bool> {
    let text = std::fs::read_to_string(path).with_context(|| format!("read {path}"))?;
    let mut report: serde_json::Map<String, Value> = serde_json::from_str(&text)?;
    let timeout = std::time::Duration::from_secs_f64(
        report["step_timeout_ms"]
            .as_f64()
            .context("report has no step_timeout_ms")?
            / 1e3,
    );
    // Every recorded child must parse, or the check could skip one and call it clean.
    let children = report["shutdown"]["children_before_shutdown"]
        .as_array()
        .context("report has no children_before_shutdown")?
        .iter()
        .map(|p| {
            let pid = p["pid"]
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .filter(|n| *n > 0)
                .with_context(|| format!("bad pid in child record {p}"))?;
            let start = p["start_time"]
                .as_u64()
                .with_context(|| format!("bad start_time in child record {p}"))?;
            Ok((pid, start))
        })
        .collect::<Result<Vec<(i32, u64)>>>()?;
    let result = probe::wait_gone(&children, timeout);
    let clean = result["clean"] == json!(true);
    let pass = report.get("scenario_pass") == Some(&json!(true)) && clean;
    report.insert("children_after_exit".into(), result);
    report.insert("shutdown_clean".into(), json!(clean));
    report.insert("pass".into(), json!(pass));
    std::fs::write(path, serde_json::to_string_pretty(&Value::Object(report))?)?;
    Ok(pass)
}

fn start(
    cx: &mut App,
    cfg: config::Config,
    pump_requests: mpsc::UnboundedReceiver<i64>,
    context_ready: mpsc::UnboundedReceiver<()>,
    report: scenario::Report,
    children: Rc<RefCell<Vec<probe::Proc>>>,
) -> Result<()> {
    // The pump starts first: CEF needs it to finish initializing.
    cx.spawn(async move |cx| pump::run(pump_requests, cx).await)
        .detach();

    const PAGE_WIDTH: i32 = 1024;
    const PAGE_HEIGHT: i32 = 640;
    let (frame_tx, frame_rx) = mpsc::unbounded();
    let (lifecycle_tx, lifecycle_rx) = mpsc::unbounded();
    let hub = Rc::new(engine::Hub {
        view_width: PAGE_WIDTH,
        view_height: PAGE_HEIGHT,
        scale_factor: Cell::new(1.0),
        frame: RefCell::new(None),
        frame_tx,
        paints: Default::default(),
        loaded: Default::default(),
        tracked: Default::default(),
        lifecycle_tx,
        render_process_terminations: Default::default(),
        paint_log: Default::default(),
    });
    let stats = Rc::new(view::ViewStats::default());
    let bounds = Bounds {
        origin: point(px(0.), px(0.)),
        size: size(
            px(PAGE_WIDTH as f32),
            px(PAGE_HEIGHT as f32 + view::HEADER_HEIGHT),
        ),
    };
    let mut options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        ..Default::default()
    };
    if cfg.no_inactive_throttle {
        options.inactive_frame_interval = None;
    }
    report.borrow_mut().insert(
        "gpui_inactive_frame_interval_ms".into(),
        json!(
            options
                .inactive_frame_interval
                .map(|d| d.as_secs_f64() * 1e3)
        ),
    );
    let window = {
        let hub = hub.clone();
        let stats = stats.clone();
        cx.open_window(options, move |window, cx| {
            hub.scale_factor.set(window.scale_factor());
            cx.new(|cx| view::PageView::new(hub, stats, frame_rx, cx))
        })?
    };
    report
        .borrow_mut()
        .insert("window_scale_factor".into(), json!(hub.scale_factor.get()));

    let run = scenario::Run {
        cfg,
        hub,
        stats,
        window,
        lifecycle: lifecycle_rx,
        context_ready,
        report,
        children_before_shutdown: children,
        navigation_started: None,
    };
    cx.spawn(async move |cx| run.run(cx).await).detach();
    Ok(())
}
