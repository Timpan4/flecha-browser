//! The scripted checks. Each scenario runs as a GPUI foreground task, writes
//! its results into the report, then closes every browser and quits GPUI.

use crate::bridge::{Bridge, CallTimeout};
use crate::config::{Config, Scenario};
use crate::engine::{self, Hub, Kind};
use crate::probe::{self, Proc};
use crate::pump;
use crate::timers::Timers;
use crate::view::{PageView, ViewStats};
use anyhow::{Context as _, Result, bail};
use cef::{ImplBrowser as _, ImplBrowserHost as _};
use cef_gpui_pump::idle;
use futures::{FutureExt as _, StreamExt as _, channel::mpsc};
use gpui::{AsyncApp, WindowHandle};
use serde_json::{Map, Value, json};
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub type Report = Rc<RefCell<Map<String, Value>>>;

/// Milliseconds between two page counter samples, on the page's own clock
/// (`performance.now()`), so page rates count only what the page did between them.
fn page_interval_ms(a: &Value, b: &Value) -> Option<f64> {
    Some(b["now"].as_f64()? - a["now"].as_f64()?).filter(|ms| *ms > 0.0)
}

pub struct Run {
    /// The scenario's own GPUI timers (steps, sleeps, the timeout probe's bound).
    pub timers: Timers,
    pub cfg: Config,
    pub hub: Rc<Hub>,
    pub stats: Rc<ViewStats>,
    pub window: WindowHandle<PageView>,
    pub lifecycle: mpsc::UnboundedReceiver<()>,
    pub context_ready: mpsc::UnboundedReceiver<()>,
    pub report: Report,
    /// Descendant processes snapshotted just before closing browsers.
    pub children_before_shutdown: Rc<RefCell<Vec<Proc>>>,
    /// When page 0's `CreateBrowserSync` started its navigation.
    pub navigation_started: Option<Instant>,
    /// Idle scenario: the measured window, judged after the pump trace stops.
    pub idle_window: Option<(Instant, Instant)>,
}

/// Counters from both sides at one instant.
struct Snap {
    at: Instant,
    pump: pump::Counts,
    renders: u64,
    animation_steps: u64,
    uploads: u64,
    overwritten: u64,
    never_drawn: u64,
    paints: Vec<u64>,
    tree: Vec<Proc>,
}

impl Run {
    fn put(&self, key: &str, value: Value) {
        eprintln!("[scenario] {key}: {value}");
        self.report.borrow_mut().insert(key.to_string(), value);
    }

    async fn sleep(&self, d: Duration) {
        self.timers.arm(d).await;
    }

    /// Waits until `pred` holds, re-checking after each lifecycle event, bounded by the step timeout.
    async fn wait_for(&mut self, what: &str, pred: impl Fn(&Hub) -> bool) -> Result<Duration> {
        let start = Instant::now();
        let deadline = start + self.cfg.step_timeout;
        loop {
            if pred(&self.hub) {
                return Ok(start.elapsed());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!(
                    "timed out after {:?} waiting for {what}",
                    self.cfg.step_timeout
                );
            }
            let mut timer = self.timers.arm(remaining).fuse();
            futures::select_biased! {
                _ = self.lifecycle.next() => {}
                _ = timer => {}
            }
        }
    }

    fn snap(&self) -> Snap {
        let pages = self.hub.paints.borrow().len();
        Snap {
            at: Instant::now(),
            pump: pump::counts(),
            renders: self.stats.renders.get(),
            animation_steps: self.stats.animation_steps.get(),
            uploads: self.stats.frames_uploaded.get(),
            overwritten: self.hub.frames_overwritten.get(),
            never_drawn: self.stats.frames_never_drawn.get(),
            paints: (0..pages).map(|i| self.hub.paints(i)).collect(),
            tree: probe::tree(probe::self_pid()),
        }
    }

    fn delta(&self, a: &Snap, b: &Snap) -> Value {
        let secs = (b.at - a.at).as_secs_f64();
        let paints: Vec<Value> = a
            .paints
            .iter()
            .zip(&b.paints)
            .map(|(x, y)| json!({ "count": y - x, "per_second": (y - x) as f64 / secs }))
            .collect();
        json!({
            "seconds": secs,
            "pump": pump::counts_json(pump::counts_delta(a.pump, b.pump)),
            "gpui_renders": b.renders - a.renders,
            "gpui_renders_per_second": (b.renders - a.renders) as f64 / secs,
            "gpui_animation_steps": b.animation_steps - a.animation_steps,
            "gpui_images_uploaded": b.uploads - a.uploads,
            "on_paint_by_page": paints,
            // Page 0's OnPaint frames that never became a GPUI image, because the
            // next OnPaint replaced them first, and images replaced before GPUI
            // drew them. Measurements only: no verdict uses them.
            "on_paint_frames_overwritten_before_upload": b.overwritten - a.overwritten,
            "on_paint_frames_overwritten_before_upload_per_second": (b.overwritten - a.overwritten) as f64 / secs,
            "gpui_images_replaced_before_draw": b.never_drawn - a.never_drawn,
            "gpui_images_replaced_before_draw_per_second": (b.never_drawn - a.never_drawn) as f64 / secs,
            "cpu": probe::cpu_delta(&a.tree, &b.tree, secs),
        })
    }

    /// The tree's references to the desktop session (see `probe::forbidden_refs`).
    fn isolation(&self) -> Value {
        probe::forbidden_refs(
            &probe::tree(probe::self_pid()),
            &self.cfg.forbidden_paths,
            &self.cfg.allowed_paths,
        )
    }

    pub async fn run(mut self, cx: &mut AsyncApp) {
        let started = Instant::now();
        // main.rs skips CefShutdown unless this turns true.
        self.put("browsers_closed", json!(false));
        let outcome = self.scenario(cx).await;
        if let Err(err) = &outcome {
            self.put("error", json!(format!("{err:#}")));
        }
        let before_shutdown = self.isolation();
        let clean = before_shutdown["clean"] == true
            && self
                .report
                .borrow()
                .get("isolation_after_load")
                .map(|v| &v["clean"])
                == Some(&json!(true));
        self.put("isolation_before_shutdown", before_shutdown);
        self.put("isolation_clean", json!(clean));
        if let Err(err) = self.shutdown(cx).await {
            self.put("shutdown_error", json!(format!("{err:#}")));
        }
        self.put("scenario_seconds", json!(started.elapsed().as_secs_f64()));
        self.put(
            "totals",
            json!({
                "gpui_renders": self.stats.renders.get(),
                "gpui_animation_steps": self.stats.animation_steps.get(),
                "gpui_images_uploaded": self.stats.frames_uploaded.get(),
                "gpui_images_dropped": self.stats.images_dropped.get(),
                "gpui_images_replaced_before_draw": self.stats.frames_never_drawn.get(),
                "on_paint_frames_overwritten_before_upload": self.hub.frames_overwritten.get(),
                "on_paint_by_page": *self.hub.paints.borrow(),
            }),
        );
        cx.update(|cx| cx.quit());
    }

    async fn start_pages(&mut self, cx: &mut AsyncApp, count: usize) -> Result<Bridge> {
        let waited = Instant::now();
        let ready = self.context_ready.next();
        let mut timer = self.timers.arm(self.cfg.step_timeout).fuse();
        futures::select_biased! {
            r = ready.fuse() => r.context("context channel closed")?,
            _ = timer => bail!("CEF context not initialized within {:?}", self.cfg.step_timeout),
        }
        self.put(
            "context_initialized_after_ms",
            json!(waited.elapsed().as_secs_f64() * 1e3),
        );
        for index in 0..count {
            if index == 0 {
                self.navigation_started = Some(Instant::now());
            }
            engine::create_page(&self.hub, index, &self.cfg.url)
                .with_context(|| format!("CreateBrowserSync failed for page {index}"))?;
        }
        let load = self
            .wait_for("page load", |hub| {
                let loaded = hub.loaded.borrow();
                loaded.len() >= count && loaded.iter().all(|l| *l)
            })
            .await?;
        self.put("page_load_ms", json!(load.as_secs_f64() * 1e3));
        let isolation = self.isolation();
        self.put("isolation_after_load", isolation);
        let bridge = Bridge::new(cx, self.cfg.step_timeout);
        for index in 0..count {
            let browser = self.hub.page(index).context("page not tracked")?;
            bridge.attach(&browser)?;
        }
        Ok(bridge)
    }

    async fn scenario(&mut self, cx: &mut AsyncApp) -> Result<()> {
        let pages = if self.cfg.scenario == Scenario::DevTools && self.cfg.second_browser {
            2
        } else {
            1
        };
        let bridge = self.start_pages(cx, pages).await?;
        // The pump is traced from page load to the end of every scenario, so a
        // late wake-up anywhere, not only in the idle window, fails the run.
        let (pump_timers_before, _) = pump::timers();
        let traced_from = Instant::now();
        pump::start_trace();
        let result = match self.cfg.scenario {
            Scenario::Idle => self.idle().await,
            Scenario::Animate => self.animate(cx, &bridge).await,
            Scenario::Cdp => self.cdp(&bridge).await,
            Scenario::DevTools => self.devtools(&bridge).await,
        };
        let trace = pump::stop_trace();
        let traced_seconds = traced_from.elapsed().as_secs_f64();
        bridge.detach_all();
        let analysis = pump::analyze(&trace);
        let max_gap_ms = analysis["gap_between_calls_ms"]["max"].as_f64();
        let step_ms = self.cfg.step_timeout.as_secs_f64() * 1e3;
        let pump_pass =
            analysis["unexplained_calls"] == 0 && max_gap_ms.is_some_and(|gap| gap <= step_ms);
        let (pump_timers_now, pump_timer_deadlines_max_unexpired) = pump::timers();
        let pump_timers = pump_timers_now - pump_timers_before;
        self.put(
            "pump_trace",
            json!({
                "seconds": traced_seconds,
                "max_gap_between_calls_ms": max_gap_ms,
                "max_gap_bound_ms": step_ms,
                "pump_timers_created": pump_timers,
                "pump_timers_per_second": pump_timers as f64 / traced_seconds,
                "pump_timer_deadlines_max_unexpired_since_start": pump_timer_deadlines_max_unexpired,
                "bridge_timers_created": bridge.timers().0,
                "scenario_timers_created": self.timers.created(),
                "scenario_timer_deadlines_max_unexpired": self.timers.max_unexpired_deadlines(),
                "analysis": analysis,
            }),
        );
        self.put("pump_pass", json!(pump_pass));
        if let Some(window) = self.idle_window {
            self.judge_idle(pump::analyze_between(&trace, Some(window)))?;
        }
        result
    }

    async fn idle(&mut self) -> Result<()> {
        self.sleep(self.cfg.warmup).await;
        let a = self.snap();
        self.sleep(self.cfg.measure).await;
        let b = self.snap();
        self.idle_window = Some((a.at, b.at));
        let delta = self.delta(&a, &b);
        self.put("idle", delta);
        Ok(())
    }

    /// The idle verdict, from the pump trace of the measured window and the
    /// page's paint log (rule and tests: `cef_gpui_pump::idle`).
    fn judge_idle(&mut self, analysis: Value) -> Result<()> {
        let (from, to) = self.idle_window.context("no idle window")?;
        let navigation = self.navigation_started.context("no navigation time")?;
        let full_view = [0, 0, self.hub.view_width, self.hub.view_height];
        let log = self.hub.paint_log.borrow().clone();
        let paints: Vec<idle::Paint> = log
            .iter()
            .map(|(t, rects)| idle::Paint {
                after_navigation: t.saturating_duration_since(navigation),
                full_view: rects.as_slice() == [full_view],
                in_window: *t >= from && *t <= to,
            })
            .collect();
        let renders = self
            .report
            .borrow()
            .get("idle")
            .and_then(|d| d["gpui_renders"].as_u64())
            .unwrap_or(u64::MAX);
        let unexplained = analysis["unexplained_calls"].as_u64().unwrap_or(u64::MAX);
        let verdict = idle::judge(&paints, renders, unexplained);
        let rows: Vec<Value> = log
            .iter()
            .zip(&paints)
            .zip(&verdict.paint_holding)
            .map(|(((_, rects), p), holding)| {
                json!({
                    "ms_after_navigation": p.after_navigation.as_secs_f64() * 1e3,
                    "in_measured_window": p.in_window,
                    "paint_holding_repaint": holding,
                    "dirty_rects": rects,
                })
            })
            .collect();
        self.put("idle_paint_log", json!(rows));
        self.put("idle_pump_trace", analysis);
        self.put(
            "idle_frames",
            json!({
                "gpui_renders_in_window": renders,
                "on_paint_in_window": verdict.paints_in_window,
                "paint_holding_repaints_in_window": verdict.paint_holding_in_window,
                "paint_holding_timeout_ms": idle::PAINT_HOLDING_TIMEOUT.as_secs_f64() * 1e3,
                "strict_no_frames": renders == 0,
            }),
        );
        self.put("idle_pass", json!(verdict.pass));
        Ok(())
    }

    async fn page_counters(&self, bridge: &Bridge) -> Result<Value> {
        let tab = bridge.tab_ids()[0];
        bridge
            .evaluate(
                tab,
                "({ raf: window.__raf || 0, now: performance.now(), css_time: (document.getAnimations()[0] || {}).currentTime ?? null, \
                   webdriver_samples: window.__wd ? window.__wd.samples : null, \
                   webdriver_true: window.__wd ? window.__wd.trues : null, \
                   webdriver_now: navigator.webdriver })",
            )
            .await
    }

    async fn animate_phase(&mut self, bridge: &Bridge) -> Result<Value> {
        self.sleep(self.cfg.warmup).await;
        let page_a = self.page_counters(bridge).await?;
        let a = self.snap();
        self.sleep(self.cfg.measure).await;
        let b = self.snap();
        let page_b = self.page_counters(bridge).await?;
        let mut delta = self.delta(&a, &b);
        let raf = page_b["raf"].as_f64().unwrap_or(0.0) - page_a["raf"].as_f64().unwrap_or(0.0);
        let page_ms = page_interval_ms(&page_a, &page_b);
        let css = match (page_a["css_time"].as_f64(), page_b["css_time"].as_f64()) {
            (Some(x), Some(y)) => json!(y - x),
            _ => Value::Null,
        };
        delta["page_raf_frames"] = json!(raf);
        delta["page_counter_interval_ms"] = json!(page_ms);
        delta["page_raf_per_second"] = json!(page_ms.map(|ms| raf / ms * 1e3));
        delta["page_css_animation_ms_advanced"] = css;
        Ok(delta)
    }

    async fn animate(&mut self, cx: &mut AsyncApp, bridge: &Bridge) -> Result<()> {
        let off = self.animate_phase(bridge).await?;
        self.window.update(cx, |view, _, cx| {
            view.gpui_animation = true;
            cx.notify();
        })?;
        let on = self.animate_phase(bridge).await?;
        let advancing = |d: &Value| {
            d["page_raf_frames"].as_f64().unwrap_or(0.0) > 0.0
                && d["page_css_animation_ms_advanced"].as_f64().unwrap_or(0.0) > 0.0
                && d["on_paint_by_page"][0]["count"].as_u64().unwrap_or(0) > 0
                && d["gpui_images_uploaded"].as_u64().unwrap_or(0) > 0
        };
        let pass = advancing(&off)
            && advancing(&on)
            && on["gpui_animation_steps"].as_u64().unwrap_or(0) > 0;
        self.put("animate_gpui_off", off);
        self.put("animate_gpui_on", on);
        self.put("animate_pass", json!(pass));
        Ok(())
    }

    async fn cdp(&mut self, bridge: &Bridge) -> Result<()> {
        let tab = bridge.tab_ids()[0];
        self.sleep(self.cfg.warmup).await;
        let tabs = bridge.list_tabs().await;
        self.put("cdp_list_tabs", json!(tabs));

        let before = self.page_counters(bridge).await?;
        let before_at = Instant::now();
        let step = self.cfg.step_timeout;
        let paints_before = self.hub.paints(0);
        let overwritten_before = self.hub.frames_overwritten.get();
        let never_drawn_before = self.stats.frames_never_drawn.get();
        // Keep the DevTools session busy for the measured window and read
        // navigator.webdriver from both the call and the page's own rAF sampler.
        let start = Instant::now();
        let flood = async {
            let mut calls = 0u64;
            let mut true_replies = 0u64;
            let mut errors = Vec::new();
            let mut latencies_ms: Vec<f64> = Vec::new();
            while start.elapsed() < self.cfg.measure {
                let t = Instant::now();
                let reply = bridge.evaluate(tab, "navigator.webdriver").await;
                latencies_ms.push(t.elapsed().as_secs_f64() * 1e3);
                match reply {
                    Ok(Value::Bool(false)) => {}
                    Ok(other) => {
                        true_replies += 1;
                        errors.push(format!("navigator.webdriver = {other}"));
                    }
                    Err(e) => errors.push(format!("{e:#}")),
                }
                calls += 1;
            }
            (calls, true_replies, errors, latencies_ms, start.elapsed())
        };
        // Alongside the loop, one call that never gets a reply (a promise that
        // never settles) shows that the bridge's timeout fires under load. It
        // must fail with the timeout within one more step timeout, measured,
        // so a reply chosen after both futures were ready cannot count.
        let timeout_probe = async {
            let deadline = step + step;
            let sent = Instant::now();
            let reply = bridge.evaluate(tab, "new Promise(() => {})").fuse();
            let mut bound = self.timers.arm(deadline).fuse();
            futures::pin_mut!(reply);
            let (timed_out, error) = futures::select_biased! {
                reply = reply => (
                    reply.as_ref().err().is_some_and(|e| e.downcast_ref::<CallTimeout>().is_some()),
                    reply.as_ref().err().map(|e| format!("{e:#}")),
                ),
                _ = bound => (false, Some("no result within two step timeouts".to_string())),
            };
            let elapsed = sent.elapsed();
            json!({
                "timed_out": timed_out,
                "within_deadline": elapsed <= deadline,
                "pass": timed_out && elapsed <= deadline,
                "error": error,
                "returned_after_ms": elapsed.as_secs_f64() * 1e3,
                "deadline_ms": deadline.as_secs_f64() * 1e3,
                "late_ms": (elapsed.as_secs_f64() - step.as_secs_f64()) * 1e3,
            })
        };
        let ((calls, true_replies, errors, mut latencies_ms, loop_time), timeout_probe) =
            futures::join!(flood, timeout_probe);
        let loop_seconds = loop_time.as_secs_f64();
        let after = self.page_counters(bridge).await?;
        let host_seconds = before_at.elapsed().as_secs_f64();
        let page_ms = page_interval_ms(&before, &after);
        let paints = self.hub.paints(0) - paints_before;
        let overwritten = self.hub.frames_overwritten.get() - overwritten_before;
        let never_drawn = self.stats.frames_never_drawn.get() - never_drawn_before;
        latencies_ms.sort_by(f64::total_cmp);
        let pct = |q: f64| {
            (!latencies_ms.is_empty())
                .then(|| latencies_ms[((latencies_ms.len() - 1) as f64 * q).round() as usize])
        };
        let latency = json!({ "p50": pct(0.5), "p99": pct(0.99), "max": pct(1.0) });
        let shot_start = Instant::now();
        let png = bridge.screenshot(tab).await;
        let shot_ms = shot_start.elapsed().as_secs_f64() * 1e3;

        let screenshot = match png {
            Ok(bytes) => {
                let path = self.cfg.out.with_extension("png");
                std::fs::write(&path, &bytes)?;
                let is_png = bytes.starts_with(b"\x89PNG\r\n\x1a\n");
                let dims = (bytes.len() >= 24)
                    .then(|| {
                        let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
                        let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
                        Some([w, h])
                    })
                    .flatten();
                json!({ "path": path, "bytes": bytes.len(), "png_signature": is_png, "size": dims, "took_ms": shot_ms })
            }
            Err(e) => json!({ "error": format!("{e:#}") }),
        };
        let samples = after["webdriver_samples"]
            .as_u64()
            .unwrap_or(0)
            .saturating_sub(before["webdriver_samples"].as_u64().unwrap_or(0));
        let page_true = after["webdriver_true"].as_u64().unwrap_or(u64::MAX);
        let tree = probe::tree(probe::self_pid());
        let switches = probe::devtools_switches(&tree);
        let sockets = probe::listening_sockets(&tree);
        let tcp_listeners = sockets["listening"]
            .as_array()
            .map(|l| l.iter().filter(|s| s["kind"] != "unix").count())
            .unwrap_or(0);
        // Both bounds come from the step timeout, the harness's bound on any one
        // wait for CEF: a call may take no longer, and the loop may overrun the
        // window by no more.
        let slowest_ms = latencies_ms.last().copied().unwrap_or(0.0);
        let step_ms = step.as_secs_f64() * 1e3;
        let loop_bound_ms = (self.cfg.measure + step).as_secs_f64() * 1e3;
        let timely = slowest_ms <= step_ms && loop_seconds * 1e3 <= loop_bound_ms;
        // The cause the latency bounds can miss: the stall is intermittent,
        // but timers piling up on GPUI's shared Timer thread are not.
        let (bridge_timers, bridge_deadlines_max_unexpired) = bridge.timers();
        let pass = calls > 0
            && timely
            && bridge_deadlines_max_unexpired <= 1
            && timeout_probe["pass"] == true
            && true_replies == 0
            && errors.is_empty()
            && samples > 0
            && page_true == 0
            && screenshot["png_signature"] == true
            && switches.is_empty()
            && sockets["complete"] == true
            && tcp_listeners == 0;
        let raf = after["raf"].as_f64().unwrap_or(0.0) - before["raf"].as_f64().unwrap_or(0.0);
        self.put(
            "cdp_evaluate_loop",
            json!({
                "calls": calls,
                "measure_ms": self.cfg.measure.as_secs_f64() * 1e3,
                "loop_seconds": loop_seconds,
                "loop_bound_ms": loop_bound_ms,
                "call_latency_ms": latency,
                "call_latency_bound_ms": step_ms,
                "timely": timely,
                "bridge_timers_created": bridge_timers,
                "bridge_timer_deadlines_max_unexpired": bridge_deadlines_max_unexpired,
                "page_raf_frames": raf,
                "page_raf_per_second": page_ms.map(|ms| raf / ms * 1e3),
                "page_counter_interval_ms": page_ms,
                "host_interval_seconds": host_seconds,
                "on_paint_frames": paints,
                "on_paint_per_second": paints as f64 / host_seconds,
                "on_paint_frames_overwritten_before_upload": overwritten,
                "on_paint_frames_overwritten_before_upload_per_second": overwritten as f64 / host_seconds,
                "gpui_images_replaced_before_draw": never_drawn,
                "webdriver_true_replies": true_replies,
                "errors": errors.iter().take(10).collect::<Vec<_>>(),
                "page_webdriver_samples_during_calls": samples,
                "page_webdriver_true_total": page_true,
                "page_before": before,
                "page_after": after,
            }),
        );
        self.put("cdp_timeout_probe", timeout_probe);
        self.put("cdp_screenshot", screenshot);
        self.put("cdp_events_received", json!(bridge.events_received()));
        self.put(
            "process_tree",
            json!(tree.iter().map(Proc::summary).collect::<Vec<_>>()),
        );
        self.put("devtools_switches_in_cmdlines", json!(switches));
        self.put("listening_sockets", sockets);
        self.put("cdp_pass", json!(pass));
        Ok(())
    }

    fn renderers() -> Vec<Proc> {
        probe::tree(probe::self_pid())
            .into_iter()
            .filter(|p| matches!(p.kind().as_str(), "renderer" | "zygote-forked"))
            .collect()
    }

    async fn page_health(&mut self, bridge: &Bridge) -> Value {
        let a = self.snap();
        self.sleep(self.cfg.measure).await;
        let b = self.snap();
        let secs = (b.at - a.at).as_secs_f64();
        let mut pages = Vec::new();
        for (index, tab) in bridge.tab_ids().into_iter().enumerate() {
            let t = Instant::now();
            let eval = bridge.evaluate(tab, "1 + 1").await;
            pages.push(json!({
                "page": index,
                "on_paint_in_window": b.paints[index] - a.paints[index],
                "on_paint_per_second": (b.paints[index] - a.paints[index]) as f64 / secs,
                "evaluate": eval.as_ref().ok(),
                "evaluate_error": eval.as_ref().err().map(|e| format!("{e:#}")),
                "evaluate_ms": t.elapsed().as_secs_f64() * 1e3,
            }));
        }
        json!(pages)
    }

    async fn devtools(&mut self, bridge: &Bridge) -> Result<()> {
        self.sleep(self.cfg.warmup).await;
        let health_before = self.page_health(bridge).await;
        let scan_cost_start = Instant::now();
        let renderers_before = Self::renderers();
        // One full /proc scan, timed here so the scan while DevTools is open
        // runs only when it fits before the close is due.
        let scan_cost = scan_cost_start.elapsed();

        // CEF #4353 times the close from the ShowDevTools() call, so this does too.
        let inspected = self.hub.page(0).context("page 0")?;
        let shown_at = Instant::now();
        if !engine::show_devtools(&self.hub, &inspected) {
            bail!("ShowDevTools: no host");
        }
        let show_call = shown_at.elapsed();
        self.wait_for("DevTools OnAfterCreated", Hub::devtools_open)
            .await?;
        let created = shown_at.elapsed();
        let devtools_style = {
            let tracked = self.hub.tracked.borrow();
            tracked
                .iter()
                .find(|t| t.kind == Kind::DevTools)
                .and_then(|t| t.browser.host())
                .map(|h| format!("{:?}", h.runtime_style()))
        };
        // Reading /proc takes time, so the tree is captured only when a scan
        // as long as the one above ends before the close is due.
        let scan_start = Instant::now();
        let tree_open = (shown_at.elapsed() + scan_cost < self.cfg.devtools_close_after)
            .then(|| probe::tree(probe::self_pid()));
        let tree_scan = scan_start.elapsed();
        let due =
            (shown_at + self.cfg.devtools_close_after).saturating_duration_since(Instant::now());
        self.sleep(due).await;
        let close_at = shown_at.elapsed();
        inspected.host().context("host")?.close_dev_tools();
        let closed = self
            .wait_for("DevTools OnBeforeClose", |h| !h.devtools_open())
            .await?;
        self.sleep(self.cfg.warmup).await;
        let health_after = self.page_health(bridge).await;
        let renderers_after = Self::renderers();

        let gone: Vec<Value> = renderers_before
            .iter()
            .filter(|p| !probe::alive(p.pid, p.start_time))
            .map(Proc::summary)
            .collect();
        let healthy = |pages: &Value| {
            pages.as_array().is_some_and(|ps| {
                ps.iter().all(|p| {
                    p["on_paint_in_window"].as_u64().unwrap_or(0) > 0 && p["evaluate"] == 2
                })
            })
        };
        let pass = healthy(&health_before)
            && healthy(&health_after)
            && self.hub.render_process_terminations.borrow().is_empty();
        self.put("devtools", json!({
            "second_browser": self.cfg.second_browser,
            "runtime_style": devtools_style,
            "show_devtools_call_ms": show_call.as_secs_f64() * 1e3,
            "devtools_created_after_show_ms": created.as_secs_f64() * 1e3,
            "close_devtools_after_show_ms": close_at.as_secs_f64() * 1e3,
            "close_late_ms": close_at.saturating_sub(self.cfg.devtools_close_after).as_secs_f64() * 1e3,
            "tree_scan_before_ms": scan_cost.as_secs_f64() * 1e3,
            "tree_scan_while_open_ms": tree_scan.as_secs_f64() * 1e3,
            "devtools_closed_after_close_ms": closed.as_secs_f64() * 1e3,
            "health_before": health_before,
            "health_after": health_after,
            "renderers_before": renderers_before.iter().map(Proc::summary).collect::<Vec<_>>(),
            "tree_while_open": tree_open.map(|t| t.iter().map(Proc::summary).collect::<Vec<_>>()),
            "renderers_after": renderers_after.iter().map(Proc::summary).collect::<Vec<_>>(),
            "renderers_before_now_gone": gone,
            "render_process_terminations": *self.hub.render_process_terminations.borrow(),
        }));
        self.put("devtools_pass", json!(pass));
        Ok(())
    }

    async fn shutdown(&mut self, cx: &mut AsyncApp) -> Result<()> {
        let me = probe::self_pid();
        *self.children_before_shutdown.borrow_mut() = probe::tree(me)
            .into_iter()
            .filter(|p| p.pid != me)
            .collect();
        let open: Vec<_> = self
            .hub
            .tracked
            .borrow()
            .iter()
            .map(|t| t.browser.clone())
            .collect();
        for browser in &open {
            if let Some(host) = browser.host() {
                host.close_browser(1);
            }
        }
        drop(open);
        // OnBeforeClose removes each browser from `tracked`. On a timeout the
        // rest stay tracked and `browsers_closed` stays false, so main.rs
        // skips CefShutdown rather than calling it with live browsers.
        let waited = self
            .wait_for("every OnBeforeClose", |h| h.open_count() == 0)
            .await;
        self.hub.frame.borrow_mut().take();
        self.window
            .update(cx, |view, window, _| view.release_images(window))?;
        let waited = waited?;
        self.put(
            "browsers_closed_after_ms",
            json!(waited.as_secs_f64() * 1e3),
        );
        self.put("browsers_closed", json!(true));
        Ok(())
    }
}
