# Spike 2: CEF message pump in GPUI, and per-browser DevTools access

Date: 2026-11-23. Pins from `rewrite/pins.toml`: CEF `158.0.3+g8c29be2+chromium-158.0.8127.18`, cef-rs `32d6bc55` (Timpan4/cef-rs), Zed/GPUI `f126950f`.

## Verdict

| Done check | Result |
|---|---|
| Pump runs only on CEF requests or CEF's documented periodic wake-up | Pass. 0 unexplained calls in 7 idle runs of 30 s |
| Static page: GPUI draws no frames | Pass after the page's one post-load repaint. Fails strictly in a window that includes it (see "Idle") |
| Idle CPU (measurement only) | 0.50 to 0.68 % of one core for the whole process tree |
| Page and GPUI animate together | Pass. Both frame counters advance |
| `navigator.webdriver === false` during CDP calls | Pass. 482,760 calls, 903 page samples, 0 true |
| CDP without a port or pipe | Pass. No `remote-debugging` switch, no listening TCP socket |
| DevTools close does not kill OSR renderers (CEF #4353) | Pass in 15 of 15 fresh processes. No workaround needed |
| Clean shutdown | Pass. `CefShutdown()` returns in 47 to 190 ms. Every child exited in all 25 runs |

## What the spike is

`cef-gpui-pump` is one binary. It is the CEF browser process and every CEF subprocess, and it opens a GPUI window (`gpui_platform::application()`). The window shows one windowless, so Alloy-style, CEF browser. Its software `OnPaint` BGRA buffer becomes a GPUI `RenderImage`. Replaced images are released with `Window::drop_image`.

- `src/pump.rs` drives `CefDoMessageLoopWork()` from GPUI's main thread. `OnScheduleMessagePumpWork` only sends the delay on a channel. A main-thread task runs the work right away for `delay <= 0`, after `min(delay, 33 ms)` for `delay > 0`, and every 33 ms when no request arrives. The 33 ms is from CEF 158 `include/cef_app.h`: "A maximum interval of approximately 33 milliseconds between calls (30 calls per second) is recommended." A thread-local flag defers a call made while one is running, as `cef_app.h` requires. Each call is logged with its reason, and `pump::analyze` checks the log.
- `src/bridge.rs` is the in-process agent bridge. It sends CDP messages with `CefBrowserHost::SendDevToolsMessage` and reads replies with an `AddDevToolsMessageObserver` registration per browser. It lists tabs (`Target.getTargetInfo`), evaluates JavaScript (`Runtime.evaluate`) and takes screenshots (`Page.captureScreenshot`). Every call has a timeout, because CEF drops malformed messages without a reply.
- `src/scenario.rs` holds the scripted checks. `src/probe.rs` reads `/proc` for the process tree, CPU time, command lines and sockets.

## How it was run

Everything ran on Xvfb (`-nolisten tcp`), with `WAYLAND_DISPLAY` unset, inside `dbus-run-session` using `scripts/isolated-session-bus.conf`. That bus has no service directories, so no portal, gvfs or a11y service starts. Each run used a fresh `mktemp` CEF cache, deleted afterwards. Chromium switches: `--ozone-platform=x11 --password-store=basic`. The sandbox was on. No test window opened on the desktop session, and the owner's Flecha install and profiles were not touched.

```sh
cd rewrite
flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
  bash ~/.claude/skills/hosts/scripts/agent-build run cargo build --release -j 14
cd spikes/cef-gpui-pump
WARMUP_MS=3000 MEASURE_MS=30000 STEP_TIMEOUT_MS=20000 \
  scripts/run-checks.sh results/2026-11-23 idle animate animate-unthrottled cdp
WARMUP_MS=3000 MEASURE_MS=2000 STEP_TIMEOUT_MS=20000 DEVTOOLS_CLOSE_MS="0 100 2000" DEVTOOLS_TRIALS=5 \
  scripts/run-checks.sh results/2026-11-23 devtools
# Idle repeats: the same command with results/2026-11-23/idle-repeat-N,
# then WARMUP_MS=6000 with results/2026-11-23/idle-warm-N.
```

The timing values are harness parameters, not product values. `STEP_TIMEOUT_MS` bounds each wait for CEF (load, a CDP reply, DevTools open and close, browser close, child exit). The runner stages a bundle in `target/release/cef-gpui-pump-bundle` (see "Cargo 1.100 and cef-rs"). Every run writes a JSON report next to its stderr in `results/2026-11-23/`.

Build: release profile. GPUI picked `NVIDIA GeForce RTX 5080 (Vulkan)` on Xvfb (`RUST_LOG=gpui_wgpu=info`, smoke run), so GPUI was not on a software renderer. CEF itself used software OSR (`OnPaint`, no shared textures).

## Pump and idle

Static page `pages/static.html`: text only, no animation, timers, focusable input or caret.

Main run (`results/2026-11-23/idle.json`, 3 s warmup after load, 30.02 s window):

| Measure | Value |
|---|---|
| Pump calls | 1181: 900 periodic, 281 for immediate requests, 0 delayed, 0 rerun |
| CEF requests | 281, all `delay_ms <= 0` |
| Unexplained calls | 0 (every periodic call came at least 33 ms after the last call and request) |
| Gap between calls | p50 33.04 ms, p99 33.10 ms, max 43.04 ms |
| Time inside `CefDoMessageLoopWork` | 67.4 ms total, p50 0.007 ms, p99 0.12 ms |
| CPU, whole tree | 0.62 % of one core (186 ms run time in 30 s); the browser process used 180 ms of it |
| GPUI renders / OnPaint | 1 / 1 |

CPU method: sum over the process tree of `/proc/PID/task/*/schedstat` run time, between snapshots at the window's edges. `utime + stime` ticks from `/proc/PID/stat` are in the reports too, and agree (0.60 %).

That run fails the strict check, because GPUI drew one frame. It drew it because CEF painted. Logging every `OnPaint` (`idle_paint_log`) in 3 repeats showed the same thing each time: one full-view repaint (`[0, 0, 1024, 640]`) 3995 to 4007 ms after the first paint, then nothing.

The likely source is Chromium's paint-holding timeout. In Chromium 158.0.8127.18, `third_party/blink/common/widget/constants.cc` sets `kNewContentRenderingDelay = base::Seconds(4)`, "the timeout for clearing old paint for a cross-document navigation". `RenderWidgetHostImpl::InitializePaintHolding` starts it at navigation, and on expiry `ClearDisplayedGraphics` calls `NotifyClearedDisplayedGraphics` on the renderer. The 4 s match is from source reading and timing, not a trace.

With the warmup moved past that timer (6 s after load), 3 of 3 runs pass: 0 GPUI renders, 0 image uploads and 0 unexplained pump calls in 30 s, at 0.68, 0.52 and 0.56 % of one core. In all 7 idle runs, GPUI renders equalled CEF `OnPaint` calls inside the window.

Not explained: some `CefDoMessageLoopWork()` calls take 10.00 ms. The report keeps the 5 slowest calls per run. In 6 of the 7 idle runs all 5 took 10.002 to 10.007 ms, and in the other run 2 did, so the full count per run is not recorded. `cef_app.h` says the call does not block. The repeating 10 ms points to a timed wait inside CEF or Chromium. It was not investigated.

## Page and GPUI animation together

Page `pages/anim.html`: a CSS `@keyframes` box and a `requestAnimationFrame` counter. Page counters were read over CDP at the window's edges. Each phase: 3 s warmup, 30 s window.

| Run | GPUI animation | Page rAF/s | CSS time advanced | OnPaint/s | GPUI renders/s | GPUI animation steps | CPU, tree |
|---|---|---|---|---|---|---|---|
| default | off | 30.01 | 30033 ms | 29.98 | 20.82 | 0 | 19.5 % |
| default | on | 30.01 | 30033 ms | 30.01 | 21.05 | 632 | 21.2 % |
| inactive throttle off | off | 30.01 | 30033 ms | 29.98 | 30.01 | 0 | 31.0 % |
| inactive throttle off | on | 30.01 | 30033 ms | 30.01 | 62.48 | 1876 | 49.5 % |

The page holds 30 frames/s in every case. 30 is CEF's default `windowless_frame_rate`, unchanged here. With default window options, GPUI draws about 21 frames/s. The window is never focused under Xvfb with no window manager, so GPUI's `inactive_frame_interval` (default 33.3 ms, "approximately 30 fps") applies. With `inactive_frame_interval: None` (`--pump-no-inactive-throttle`), GPUI shows every page frame (30/s), and runs at 62/s while its own animation runs. Its CPU numbers include those extra frames. On a focused window on a real session, the default throttle should not apply. That is unverified here.

## Agent access through `SendDevToolsMessage`

`results/2026-11-23/cdp.json`:

- List tabs: 1 tab, `Target.getTargetInfo` returned `type: page`, title `anim`, the file URL.
- Evaluate: `Runtime.evaluate("navigator.webdriver")` back to back for 30.13 s: 482,760 calls, every reply `false`, no errors. The page counted `navigator.webdriver` itself on every animation frame: 903 samples during those calls, 0 not `false`.
- Screenshot: `Page.captureScreenshot` returned a 1024 × 640 PNG (`cdp.png`) in 107 ms.
- No port or pipe: no process in the tree has `remote-debugging` or `enable-automation` on its command line. No process in the tree has a listening TCP socket, and every fd table was readable. The one listening socket is Chromium's ProcessSingleton Unix socket (`/tmp/org.chromium.Chromium.*/SingletonSocket`), which is not a DevTools endpoint.

## DevTools and CEF #4353

CEF #4353: with every browser windowless and the app on `CefDoMessageLoopWork()`, closing a DevTools window started browser shutdown and killed renderers. The issue's own results: a first close 0 to 200 ms after `ShowDevTools` froze every page, and a close at 1 to 3 s froze the uninspected page. CEF fixed it in `11d50773` ("acquire an application keep-alive on the first CefDoMessageLoopWork call"). The pinned build `8c29be2` is 6 commits ahead of `11d50773` and 0 behind (GitHub compare).

Test: 2 windowless browsers on the animation page. Page 0 is inspected, page 1 is not. `ShowDevTools` gets a default (windowed) `CefWindowInfo` and a real client, `CloseDevTools()` runs N ms after the `ShowDevTools` call, and then each page is checked: `OnPaint` rate over 2 s, `Runtime.evaluate("1 + 1")`, renderer processes, `OnRenderProcessTerminated`. 5 fresh processes per delay.

| Close delay | Close after `ShowDevTools` | Pages' OnPaint/s after | Evaluate after | Renderer terminations |
|---|---|---|---|---|
| 0 ms | 36 to 70 ms | 29 to 30, both pages | 2 and 2, all trials | 0 |
| 100 ms | 100 to 106 ms | 29 to 30, both pages | 2 and 2, all trials | 0 |
| 2000 ms | 2000 ms | 29 to 30, both pages | 2 and 2, all trials | 0 |

The DevTools browser was `CEF_RUNTIME_STYLE_CHROME` in its own X11 window. `ShowDevTools()` created it synchronously (the call took 35 to 70 ms). While it was open, the tree had 6 zygote-forked processes, against 5 before. In every trial exactly one of the 5 earlier ones was gone afterwards and a new one took its place. Both pages kept painting and answering, so that one was not a page renderer. Most likely it was the spare renderer that DevTools took over. The data does not prove that.

Result: #4353 does not reproduce on CEF 158.0.3, and no workaround is needed. The check was not run against a build without the fix (for example CEF 156), so its ability to catch the freeze is not shown here.

## Shutdown

Each run closes every browser with `CloseBrowser(true)`, waits for every `OnBeforeClose`, drops all `CefBrowser` references and observer registrations, quits GPUI, and then calls `CefShutdown()`. `CefShutdown()` returned in 47 to 190 ms. Right after it returns, some zygote processes are still alive. They exit when the browser process exits and closes their sockets. So after the app exits, the runner calls `cef-gpui-pump --pump-wait-gone=REPORT`. It waits on a pidfd for each child the app saw before shutdown, bounded by `STEP_TIMEOUT_MS`. In all 25 runs, every child had exited before that wait began.

Process types come from `--type=`, but zygote-forked processes keep `--type=zygote` on this build. `probe::Proc::kind` labels them by tree shape. The sandboxed layout seen here is browser, then the sandboxed zygote (PID-namespace init), then the zygote worker, then one process per renderer, each in its own PID namespace.

## Cargo 1.100 and cef-rs

Cargo 1.100's default build-dir layout puts `OUT_DIR` at `target/release/build/cef-dll-sys/<hash>/out`. cef-dll-sys copies CEF's runtime files to the third parent of that, `target/release/build`, not next to the binary (tauri-apps/cef-rs#522, open). Workaround: `scripts/run-checks.sh` builds `target/release/cef-gpui-pump-bundle` from the binary and the CEF distribution in cef-dll-sys's `OUT_DIR`, whose `archive.json` must name the pinned version. The binary has `RPATH=$ORIGIN` (`build.rs`).

The archive used is `cef_binary_158.0.3+g8c29be2+chromium-158.0.8127.18_linux64_minimal.tar.bz2`, SHA-1 `620b80a02dfe93ed7884a9eb1b265ff34f799024`. That is the stable-channel archive of the pinned commit (`pins.toml` `[cef.stable.sha1]`), as the plan's beta archive gap says.

## Dependencies

- cef-rs without `accelerated_osr`: its registry `wgpu = "30"` is optional and not in the graph. `Cargo.lock` has one wgpu, Zed's git rev `cddb2f5d`. No conflict for this spike.
- `gpui_platform` needs `features = ["x11", "wayland"]`. Zed's workspace turns off `gpui_linux`'s default backends, and without the features `gpui_linux` builds with no display backend.
- `rewrite/Cargo.toml` copies Zed's `[patch.crates-io]` entries for crates GPUI uses (`async-task`, `async-process`, `calloop`, `khronos-egl`). Zed's patches do not apply to git dependents.

## Not verified

- Wayland, real X11 with a window manager, macOS and Windows. Everything ran on Xvfb.
- GPUI frame pacing on a focused window. Under Xvfb the window stays inactive.
- That the 4 s repaint is `kNewContentRenderingDelay`. It matches source and timing, with no trace.
- The 10 ms `CefDoMessageLoopWork()` calls.
- That the #4353 check fails on a build without the fix.
- Input, IME, resize and DPI changes. They are not in this spike's done check.
- The plan's beta archive. The build used the stable archive of the same commit.
