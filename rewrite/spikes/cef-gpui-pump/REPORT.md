# Spike 2: CEF message pump in GPUI, and per-browser DevTools access

Date: 2026-11-23. Pins from `rewrite/pins.toml`: CEF `158.0.3+g8c29be2+chromium-158.0.8127.18` (beta-channel minimal archive), cef-rs `32d6bc55` (Timpan4/cef-rs), Zed/GPUI `f126950f`. cef-rs and Zed are the sources Spike 1's `prepare.sh` stages and patches (see "Dependencies").

Current results: `results/2026-11-23-r5/`, one run of `scripts/run-checks.sh` as committed, on the head that merges main at `04b013d` (see "How it was run"). `results/2026-11-23-r5-cdp-repeats/` holds 3 more cdp runs with the browser's threads sampled, and `results/2026-11-23-r5-controls/` the negative controls of this round. `build.txt` in each names the commit and `cef-archive.json` the CEF archive. History, cited only where this report says so: `results/2026-11-23/` is the first round, `results/2026-11-23-r3/` the first fix round (its `review-evidence/` holds that round's repro inputs and outputs, not to be rerun), and `results/2026-11-23-r4/` the second. r4 used the CDP bridge that starved the pump (see "The CDP slowdown").

## Verdict

| Done check | Result |
|---|---|
| Pump runs only on CEF requests or CEF's documented periodic wake-up | Pass. 0 unexplained calls in every run. The pump trace now covers every scenario, and the largest gap between calls was 88.8 ms |
| Static page: GPUI draws no frames | Pass, with one explained frame. GPUI drew 1 frame for CEF's one paint-holding repaint at 4099 ms after navigation, and nothing else (see "Pump and idle") |
| Idle CPU (measurement only) | 0.53 % of one core for the whole process tree |
| Page and GPUI animate together | Pass. Both frame counters advance |
| `navigator.webdriver === false` during CDP calls | Pass. 786,719 calls in 30.000 s, 900 page samples, 0 true. A control run with `--enable-automation` fails |
| CDP calls do not stall the page or the pump | Pass in 4 of 4 cdp runs after the fix. Before it, the bridge's per-call timers starved the pump (see "The CDP slowdown"). A control with the old timers fails |
| CDP without a port or pipe | Pass. No `remote-debugging` switch, no listening TCP socket in any of the tree's network namespaces, every fd and socket table read |
| DevTools close does not kill OSR renderers (CEF #4353) | Pass in 15 of 15 fresh processes. No workaround needed |
| Clean shutdown | Pass. `CefShutdown()` returns in 48 to 117 ms. Every child exited in all 19 runs |
| Runs isolated from the desktop session and the owner's home | Pass in 19 of 19 runs. Controls with the first round's environment, and with the scan pointed at the scratch home, fail |

## What the spike is

`cef-gpui-pump` is one binary. It is the CEF browser process and every CEF subprocess, and it opens a GPUI window (`gpui_platform::application()`). The window shows one windowless, so Alloy-style, CEF browser. Its software `OnPaint` BGRA buffer becomes a GPUI `RenderImage`. Images GPUI drew are released with `Window::drop_image` once replaced. Frames replaced before GPUI drew them are dropped at once.

The library (`src/lib.rs`, no CEF, unit-tested) holds the rules the checks depend on:

- `src/schedule.rs` decides when to call `CefDoMessageLoopWork()`. It calls right away for `delay <= 0`, after the delay for `delay > 0`, and no later than 33 ms after the previous call ended. A new request replaces the pending one, as `OnScheduleMessagePumpWork` specifies, but cannot push the 33 ms deadline back. A request made before the last call started was served by that call. The 33 ms is from CEF 158 `include/cef_app.h`: "A maximum interval of approximately 33 milliseconds between calls (30 calls per second) is recommended." A guard defers a call made while one is running until it returns, as `cef_app.h` requires, and is released when the call unwinds. `analyze` checks a call log against those rules.
- `src/idle.rs` is the idle check's frame rule (see "The one frame: paint holding").

The binary:

- `src/pump.rs` wires the schedule to CEF and GPUI. `OnScheduleMessagePumpWork` only sends the delay, stamped with the time CEF asked, on a channel. A GPUI main-thread task owns the schedule, makes the guarded call, and logs each call with its reason. It arms a GPUI timer only when a request is due earlier than the timer already armed.
- `src/bridge.rs` is the in-process agent bridge. It sends CDP messages with `CefBrowserHost::SendDevToolsMessage` and reads replies with an `AddDevToolsMessageObserver` registration per browser. It lists tabs (`Target.getTargetInfo`), evaluates JavaScript (`Runtime.evaluate`) and takes screenshots (`Page.captureScreenshot`). Every call has a deadline, because CEF drops malformed messages without a reply. One sweeper task fails calls past their deadline and holds at most one GPUI timer.
- `src/scenario.rs` holds the scripted checks. `src/probe.rs` reads `/proc` for the process tree, CPU time, command lines, sockets and references to the desktop session and the owner's home.

## How it was run

```sh
cd rewrite
bash spikes/gpui-cef-compositor/prepare.sh linux64
CARGO_NET_GIT_FETCH_WITH_CLI=true flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
  bash ~/.claude/skills/hosts/scripts/agent-build run cargo build --locked --release -j 14 -p cef-gpui-pump
flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
  bash ~/.claude/skills/hosts/scripts/agent-build run cargo test --locked --release -j 14 -p cef-gpui-pump
cd spikes/cef-gpui-pump
export WARMUP_MS=3000 MEASURE_MS=30000 STEP_TIMEOUT_MS=20000
DEVTOOLS_CLOSE_MS="0 100 2000" DEVTOOLS_TRIALS=5 scripts/run-checks.sh results/2026-11-23-r5
for i in 1 2 3; do scripts/run-checks.sh results/2026-11-23-r5-cdp-repeats/$i cdp; done
```

The runner exits 0 only if every run passes. It ran as a transient `systemd-run --user` unit and printed `all runs passed` for the full suite and each repeat (`run.log` in each dir). `build.txt` names `21c9ef3`; the head after it changes only the controls' files. The timing values are harness parameters, not product values. `STEP_TIMEOUT_MS` bounds each wait for CEF (Xvfb start, load, a CDP reply, DevTools open and close, browser close, child exit), and since this round also the longest CDP call, the CDP loop's overrun and the longest gap between pump calls. The runner stages a bundle in `target/release/cef-gpui-pump-bundle/<archive name>` (see "Cargo 1.100 and cef-rs"). Every run writes a JSON report, its stderr, and a listing of its scratch dirs.

Build: release profile. GPUI picked `NVIDIA GeForce RTX 5080 (Vulkan)` on Xvfb, so GPUI was not on a software renderer (`results/2026-11-23-r4/adapter-smoke/`, a 5 s idle run with `RUST_LOG=gpui_wgpu=info` on the r4 build). CEF itself used software OSR (`OnPaint`, no shared textures). Chromium switches: `--ozone-platform=x11 --password-store=basic`. The sandbox was on.

### Isolation

Each app run gets one `mktemp -d` base (mode 700), deleted afterwards, which holds:

- the CEF cache;
- `XDG_RUNTIME_DIR`;
- `HOME`;
- `TMPDIR`.

The run also gets:

- Xvfb (`-nolisten tcp`) on a display it picks, with `WAYLAND_DISPLAY` and `WAYLAND_SOCKET` unset.
- `XDG_CONFIG_HOME`, `XDG_CACHE_HOME`, `XDG_DATA_HOME`, `XDG_STATE_HOME`, `MESA_SHADER_CACHE_DIR` and `__GL_SHADER_DISK_CACHE_PATH` unset, so nothing points back into the real home.
- `dbus-run-session` with a config written from `scripts/isolated-session-bus.conf.in`: no service directories, so nothing is activated, and `<listen>unix:dir=SCRATCH</listen>`, so dbus-daemon creates a socket with a new random name in that run's scratch runtime dir. The first fix round listened at `$XDG_RUNTIME_DIR/bus` (`unix:runtime=yes`). A test with two buses on one scratch dir showed that listener replacing an existing socket at its path and deleting it on exit, so pointed at the desktop's runtime dir it would remove `/run/user/1000/bus`. `scripts/check-bus-listener.sh` starts a second bus on the same dir while the first runs and requires the first socket to be the same file and still answering afterwards. The runner runs it before any app run. It passes with the new listener and fails with the old one ("second bus reused …/bus", "first bus socket … is gone").
- `DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent`.

The app checks this itself. After the page loads and again before shutdown, `probe::forbidden_refs` lists every file the process tree has open or memory-mapped and every Unix socket peer (from `ss -xan`). It fails the run if any lies under the caller's `XDG_RUNTIME_DIR` (`/run/user/1000`), `/run/dbus/system_bus_socket`, or the caller's real `HOME`, except the bundle, the test pages and the results dir, which the runner passes as allowed (`--pump-allow-paths`). Paths match by whole components. In r5 all 38 samples were clean, with every fd table and map readable and no `ss` error. The limits: it samples twice, so a connection opened and closed between samples is missed, and sandboxed renderers live in their own network namespace, so their sockets (12 to 22 per sample) get no peer path from `ss`. Their open and mapped files are still scanned.

What used to reach the desktop session, the owner's home and the shared `/tmp` now lands in the scratch dirs (`*.scratch-dirs.txt`, a listing taken after each run): `runtime/dconf/user`, Chromium's NSS db in `home/.local/share/pki/nssdb`, and Mesa, RADV and NVIDIA shader caches in `home/.cache`. Chromium's ProcessSingleton socket sits under the scratch `TMPDIR` and is gone by the time of the listing.

Negative controls:

- One idle run with the first round's environment (the caller's `XDG_RUNTIME_DIR`, no system bus override, the old bus config listening in `/tmp`; `results/2026-11-23-r3/review-evidence/isolation-control/`). It failed `isolation_clean`. The browser process had `/run/user/1000/dconf/user` and `/run/user/1000/dconf/cosmic` mapped and 4 connections to `/run/dbus/system_bus_socket`.
- One idle run through the runner with `home-scan-control.patch`, which adds the run's scratch `HOME` to the forbidden paths (`results/2026-11-23-r5-controls/4-home-scan-detects/`). It failed `isolation_clean` on `home/.cache/mesa_shader_cache/index` and `home/.cache/radv_builtin_shaders/index`, mapped by sandboxed processes. A run with the real `HOME` would map the owner's copies, as the PR verifier saw for `~/.cache/mesa_shader_cache/index`, and fail the same way.

The first round's very first smoke run used stock `dbus-run-session`, which activated a private gvfs daemon, xdg-desktop-portal, the document portal, the gtk and cosmic portal backends and the a11y bus. gvfs failed to mount `/run/user/1000/gvfs` (permission denied). No process from that run survives, and the desktop session's gvfs mount is intact.

Related host state, not caused by this spike as far as the journal shows: the desktop session's `xdg-document-portal.service` exited with status 21 at 08:41:56 and stayed failed, so `/run/user/1000/doc` was not mounted. The first smoke run's Xvfb started at 08:42:00, after that exit. The portal processes logging in that second (`gnome-keyring-daemon` 23676, `xdg-desktop-portal-cosmic` 23705) ran in another agent session's systemd scope. The cause is undetermined. With the owner's approval the service was restarted. At 10:41:30 CET it became active with exit status 0, and `/run/user/1000/doc` is mounted again (`fuse.portal`).

No test window opened on the desktop session, and the owner's Flecha install and profiles were not touched.

## Pump and idle

Static page `pages/static.html`: text only, no animation, timers, focusable input or caret. `results/2026-11-23-r5/idle.json`, 3 s warmup after load, 30 s window:

| Measure | Value |
|---|---|
| Pump calls | 1,188: 902 periodic, 286 for immediate requests, 0 delayed, 0 rerun |
| CEF requests | 287, all `delay_ms <= 0` |
| Unexplained calls | 0 (each periodic call came at least 33 ms after the previous call ended) |
| Gap between calls | p50 33.03 ms, p99 33.08 ms, max 43.06 ms |
| Time inside `CefDoMessageLoopWork` | 52.0 ms total, p50 0.004 ms, p99 0.13 ms, max 10.003 ms |
| CPU, whole tree | 0.53 % of one core (159 ms run time in 30 s) |
| GPUI renders / OnPaint in the window | 1 / 1, the paint-holding repaint |

CPU method: sum over the process tree of `/proc/PID/task/*/schedstat` run time, between snapshots at the window's edges. `utime + stime` ticks from `/proc/PID/stat` are in the report too.

Each request now carries the time `OnScheduleMessagePumpWork` ran, not the time the pump task dequeued it, so a request that waits while GPUI is busy shows that wait in the trace.

### The pump trace in every scenario

Every scenario now records the pump trace from page load to the end of the scenario (`pump_trace` in each report), and `pump_pass` requires 0 unexplained calls and no gap between calls longer than `STEP_TIMEOUT_MS`. The idle check analyses only the calls in its window, with the earlier trace kept as history, so a request made just before the window explains a call just after it.

| Run | Pump calls | Unexplained | Max gap between calls | Pump timers armed per second |
|---|---|---|---|---|
| idle | 1,421 | 0 | 43.7 ms | 30.5 |
| animate | 7,533 | 0 | 58.4 ms | 41.3 |
| animate-unthrottled | 7,200 | 0 | 41.9 ms | 44.8 |
| cdp | 788,089 | 0 | 44.4 ms | 30.7 |
| devtools, 15 runs | 10,012 to 13,014 | 0 | 88.8 ms (largest of the 15) | 38.2 to 47.6 |
| cdp repeats, 3 runs | 726,848 to 750,797 | 0 | 44.4 ms (largest of the 3) | 30.4 to 30.7 |

Pump calls here count the whole scenario, from page load on. The idle table above counts the 30 s window only.

The CDP freeze (see "The CDP slowdown") is the late wake-up the idle-only trace missed. In controls 1 and 3 the largest gaps were 11,333 and 10,945 ms. Both are under `STEP_TIMEOUT_MS`, so `pump_pass` alone did not fail them either.

### The one frame: paint holding

The one paint is a full-view repaint (`[0, 0, 1024, 640]`) 4099 ms after navigation. The first round saw it in 4 of 4 runs with a 3 s warmup, 3995 to 4007 ms after the first paint, and never with a 6 s warmup.

It is Chromium's paint-holding timeout. In the local Chromium 158.0.8119.0 tree, `third_party/blink/common/widget/constants.cc:15` sets `kNewContentRenderingDelay = base::Seconds(4)`. `RenderWidgetHostImpl::InitializePaintHolding` starts it at navigation, and on expiry `ClearDisplayedGraphics` makes the renderer repaint once. The PR verifier confirmed the cause: a CDP `Input.dispatchMouseEvent` about 1.5 s after load moved the repaint to 1503 ms and removed the 4 s paint. That path is `content/browser/devtools/protocol/input_handler.cc:694`, which calls `ForceFirstFrameAfterNavigationTimeout()`.

The parent agent's decision: the idle check passes with that frame explained. The rule is `idle::judge` in the library. The paint-holding repaint is the navigation's first full-view paint at least `kNewContentRenderingDelay` after navigation, counted over every paint since navigation, not only the window. `idle_pass` requires 0 unexplained pump calls in the window, every paint in the window to be that repaint, and no more GPUI renders than CEF paints in the window. `idle_frames.strict_no_frames` records whether the window had no frame at all.

The second fix round counted the allowance inside the window only, so a lone full-view repaint at 8 s passed when the real repaint fell before the window. Its rule also lived in the binary, which has no tests. See "Unit tests" for the cases and mutants.

### 10 ms calls: CEF's pump time slice

The slowest `CefDoMessageLoopWork()` calls take 10.00 ms (10.003 ms max in r5). That is CEF's external pump time slice. CEF 8c29be2 `libcef/browser/browser_message_loop.cc:107` creates `MessagePumpExternal(0.01f, handler)`, and its `Run()` (lines 29 to 49) keeps running work until the next task is more than `max_time_slice_` (10 ms) past the call's start. When a delayed task is due within 10 ms, the call waits for it. So one pump call can hold GPUI's main thread for up to 10 ms, which is most of a 16.7 ms frame at 60 Hz. Phase 1's frame budget has to allow for it.

### Why CEF never asks for delayed work

`requests_delayed` is 0 in every run, by design. Chromium's `ThreadControllerWithMessagePumpImpl::SetNextDelayedDoWork` (`base/task/sequence_manager/thread_controller_with_message_pump_impl.cc:199` to `224`) only schedules delayed work when the thread is not inside `DoWork`, and nearly all delayed tasks are posted from inside it. CEF timers therefore rely on the 33 ms periodic call. The `delay > 0` path and the reentrancy guard are covered by unit tests (see "Unit tests").

## Page and GPUI animation together

Page `pages/anim.html`: a CSS `@keyframes` box and a `requestAnimationFrame` counter. Page counters were read over CDP at the window's edges. Each phase: 3 s warmup, 30 s window.

| Run | GPUI animation | Page rAF/s | CSS time advanced | OnPaint/s | GPUI renders/s | GPUI animation steps | CPU, tree |
|---|---|---|---|---|---|---|---|
| default | off | 30.04 | 30066 ms | 30.01 | 20.85 | 0 | 17.6 % |
| default | on | 30.01 | 30033 ms | 29.98 | 20.92 | 628 | 18.1 % |
| inactive throttle off | off | 30.04 | 30066 ms | 30.01 | 30.01 | 0 | 23.9 % |
| inactive throttle off | on | 30.01 | 30033 ms | 29.98 | 62.46 | 1875 | 46.2 % |

The page holds 30 frames/s in every case. `include/internal/cef_types.h` gives `windowless_frame_rate` "the default value is 30", and the spike does not change it. With default window options, GPUI draws about 21 frames/s. The window is never focused under Xvfb with no window manager, so GPUI's `inactive_frame_interval` (default 33.3 ms, "approximately 30 fps") applies. With `inactive_frame_interval: None` (`--pump-no-inactive-throttle`), GPUI shows every page frame, and draws more while its own animation runs. Its CPU numbers include those extra frames. On a focused window on a real session, the default throttle should not apply. That is unverified here.

## Agent access through `SendDevToolsMessage`

`results/2026-11-23-r5/cdp.json`:

- List tabs: 1 tab, `Target.getTargetInfo` returned `type: page`, title `anim`, the file URL.
- Evaluate: `Runtime.evaluate("navigator.webdriver")` back to back. The loop ran 30.000 s for a 30 s window: 786,719 calls, every reply `false`, no errors. Per-call latency: p50 0.029 ms, p99 0.11 ms, max 43.0 ms. The page counted `navigator.webdriver` on every animation frame: 900 samples during the loop, 0 not `false`, and its rAF rate over the loop was 30.00 frames/s.
- Timeout under load: while the loop runs, a second call evaluates `new Promise(() => {})` with `awaitPromise`, which never replies. The bridge failed it after 20000.0 ms, 0.03 ms past its 20 s deadline. `cdp_pass` requires it to time out.
- Screenshot: `Page.captureScreenshot` returned a 1024 × 640 PNG (`cdp.png`) in 105 ms.
- Negative control (`results/2026-11-23/cdp-enable-automation/`, first-round build): the same check with `EXTRA_ARGS=--enable-automation`, 3 s warmup and a 5 s window. All 58,080 replies and all 153 page samples were `true`, and the run failed. The check detects `navigator.webdriver` turning on.
- No port or pipe: no process in the tree has `remote-debugging` or `enable-automation` on its command line. No process has a listening TCP socket. The check reads `/proc/PID/net/{tcp,tcp6,unix}` once per network namespace in the tree (the `/proc/PID/ns/net` link tells them apart), so sandboxed renderers' namespaces are covered. In each cdp run it read 2 namespaces. The run fails unless every fd table and socket table was read (`listening_sockets.complete`), and in r5 all were. The one listening socket is Chromium's ProcessSingleton Unix socket (`$TMPDIR/org.chromium.Chromium.*/SingletonSocket`, under the run's scratch `TMPDIR`), which is not a DevTools endpoint.

`cdp_pass` also requires the loop to end within the window plus `STEP_TIMEOUT_MS` (50 s here), every call to return within `STEP_TIMEOUT_MS`, and the bridge never to hold more than one GPUI timer whose deadline has not passed (`bridge_timers_max_outstanding`). This run's bridge armed 3 timers, with at most 1 outstanding.

## The CDP slowdown

The first round reported 30.13 s for the 30 s loop, measured after the screenshot and the `/proc` scans. The PR verifier then saw 47.3, 33.4 and 34.7 s from that field, with 903, 817 and 818 page frames. r3 and r4 recorded the loop alone and ran 30.006 s and 30.000 s, and the second fix round reported that the slowdown did not reproduce. It does. The verifier ran the r4 code (a7a92bd) 5 times: the loop took 49 to 57 s, the worst call 22 to 30 s, and the page dropped to 14 to 16 frames/s, while `cdp_pass` stayed true.

### Cause

GPUI timers on Linux all run on one shared thread and are not cancelled when dropped:

1. `BackgroundExecutor::timer(d)` with a nonzero `d` goes through `dispatcher.dispatch_after` (`zed/crates/gpui/src/platform_scheduler.rs:129-150`).
2. On Linux that inserts a calloop `Timer` source into the event loop of the single `Timer` thread (`gpui_linux/src/linux/dispatcher.rs:54-90`).
3. Dropping the returned `Task` does not remove the source. It stays until its deadline and then fires into nothing.

The r4 bridge raced every call against a fresh 20 s `executor.timer(timeout)`. At about 15,000 calls a second, that kept about 300,000 timers on the Timer thread. The verifier sampled the browser's threads once a second: about 5 s into the loop the Timer thread reached 100 % of a core, and about 27 s in the main thread sat idle in `epoll_wait`. The pump's own 33 ms timer was stuck behind that backlog, so CEF went unpumped. The bridge's timeout could not fire either, because its timer sat in the same queue. With the per-call timer replaced by a future that never fires, the loop ran 30.000 s with 561,307 calls, a 35 ms worst call and 30.0 page frames/s.

### Fix

- `src/bridge.rs`: a call records a deadline instead of arming a timer. One sweeper task sleeps until the earliest deadline in flight, or until a call arrives when none is, and fails the calls past their deadline. Deadlines are the call time plus one fixed timeout, so a call made while the timer is set never has an earlier deadline, and the sweeper arms about one timer per timeout. Every bridge timer goes through one ledger, `Timers::arm`, which counts those created and the most outstanding at once.
- `src/pump.rs`: the pump re-armed a timer for every request. It now keeps the armed timer while a request is not due sooner, which brings it to about 30 timers a second.

### Measurements

The Timer thread's CPU, sampled once a second as the verifier did (`threads.log`; 100 ticks are one core):

| Run | Build | Loop | Worst call | Page frames/s | Bridge timers created / max outstanding | Timer thread | `cdp_pass` |
|---|---|---|---|---|---|---|---|
| r5 | fixed | 30.000 s | 43.0 ms | 30.00 | 3 / 1 | not sampled | true |
| Repeat 1 | fixed | 30.000 s | 27.8 ms | 30.03 | 3 / 1 | no ticks in 33 samples | true |
| Repeat 2 | fixed | 30.000 s | 19.0 ms | 30.03 | 3 / 1 | no ticks in 33 samples | true |
| Repeat 3 | fixed | 30.000 s | 43.1 ms | 30.00 | 3 / 1 | active in 1 of 33 samples, 1 tick | true |
| Control 1 | per-call timer, before the ledger | 38.325 s | 11316.6 ms | 21.21 | not counted | active in 38 of 42 samples, max 102, mean 87 ticks/s | true |
| Control 2 | per-call timer and a pump timer per request, before the ledger | 30.004 s | 33.4 ms | 30.0 | not counted | active in 29 of 33 samples, max 103, mean 97 ticks/s | true |
| Control 3 | per-call timer through the ledger | 37.937 s | 10936.5 ms | 21.42 | 503,445 / 374,654 | active in 37 of 50 samples, max 102, mean 84 ticks/s | false |

Controls are in `results/2026-11-23-r5-controls/`, each run once through the runner with its full isolation. Each applies its patch from that dir to the committed code.

The bounds the parent asked for, a call over `STEP_TIMEOUT_MS` or a loop over the window plus `STEP_TIMEOUT_MS`, did not catch the controls here. The stalls on this host were shorter than the verifier's (worst calls of 11 s against 22 to 30 s) and intermittent: control 2 saturated the Timer thread and still ran on time. So `cdp_pass` also checks the cause directly: the bridge may never hold more than one outstanding timer, which is the sweeper's design. Control 3 fails on that and passes the latency bounds. Why r4's single run did not stall is not known.

### Phase 1 risk

GPUI timers on Linux share one thread and are not cancelled on drop. Any per-request timer pattern in Flecha, such as timeouts in the MCP server, CDP calls or downloads, must not create one GPUI timer per request. Keep one timer per component, set for the earliest deadline, as the bridge does now, or use a timer source that removes cancelled timers. A Timer thread backlog delays every GPUI timer in the process, including the CEF pump's.

## DevTools and CEF #4353

CEF #4353: with every browser windowless and the app on `CefDoMessageLoopWork()`, closing a DevTools window started browser shutdown and killed renderers. The issue's own results: a first close 0 to 200 ms after `ShowDevTools` froze every page, and a close at 1 to 3 s froze the uninspected page. CEF fixed it in `11d50773` ("acquire an application keep-alive on the first CefDoMessageLoopWork call"). The pinned build `8c29be2` is 6 commits ahead of `11d50773` and 0 behind (GitHub compare).

Test: 2 windowless browsers on the animation page. Page 0 is inspected, page 1 is not. `ShowDevTools` gets a default (windowed) `CefWindowInfo` and a real client, `CloseDevTools()` runs N ms after the `ShowDevTools` call, and then each page is checked: `OnPaint` rate over 30 s, `Runtime.evaluate("1 + 1")`, renderer processes, `OnRenderProcessTerminated`. 5 fresh processes per delay.

| Close delay | Close after `ShowDevTools` | `ShowDevTools` call | Pages' OnPaint/s after | Evaluate after | Renderer terminations |
|---|---|---|---|---|---|
| 0 ms | 38 to 41 ms | 38 to 41 ms | 30, both pages | 2 and 2, all trials | 0 |
| 100 ms | 100 to 103 ms | 37 to 38 ms | 30, both pages | 2 and 2, all trials | 0 |
| 2000 ms | 2000 ms | 37 to 39 ms | 30, both pages | 2 and 2, all trials | 0 |

`ShowDevTools()` is synchronous: it returns after creating the DevTools browser, and that took up to 41 ms in r5 (up to 624 ms in r3). A close requested sooner than that runs when the call returns, so the 0 ms delay lands late by the length of that call. The DevTools browser was `CEF_RUNTIME_STYLE_CHROME` in its own X11 window. The `/proc` scan while DevTools is open runs only if a scan as long as the one timed just before fits before the close. In r5 the scan was skipped in the 5 trials with a 0 ms delay and ran in the other 10. Those scans took 19 to 23 ms for the 100 ms delay and 17 to 19 ms for the 2000 ms delay, and each ended before the close was due. Those 10 closes still landed 0.04 to 2.6 ms late. The wait resumes on GPUI's main thread, where a single pump call can run for up to 10 ms, which likely accounts for that lateness.

Result: #4353 does not reproduce on CEF 158.0.3, and no workaround is needed. The check was not run against a build without the fix (for example CEF 156), so its ability to catch the freeze is not shown here.

## Shutdown

Each run closes every browser with `CloseBrowser(true)` and waits for every `OnBeforeClose`, which removes that browser's handle. Then it drops the observer registrations, quits GPUI, and calls `CefShutdown()`. If any browser is still open when the wait times out, the app skips `CefShutdown()` (calling it with live browsers is not allowed), records why, and fails the run. `CefShutdown()` returned in 48 to 117 ms. Right after it returns, some zygote processes are still alive. They exit when the browser process exits and closes their sockets. So after the app exits, the runner calls `cef-gpui-pump --pump-wait-gone=REPORT` under `env -i` with the run's scratch `HOME` and `TMPDIR`. It waits on a pidfd for each child the app saw before shutdown, bounded by `STEP_TIMEOUT_MS`. A malformed child record, or a `/proc` or `pidfd_open` error other than "no such process", fails the check instead of counting as gone. In all 19 runs, every child had exited before that wait began.

Process types come from `--type=`, but zygote-forked processes keep `--type=zygote` on this build. `probe::Proc::kind` labels them by tree shape. The sandboxed layout seen here is browser, then the sandboxed zygote (PID-namespace init), then the zygote worker, then one process per renderer, each in its own PID namespace.

## Unit tests

`cargo test --release -p cef-gpui-pump` runs 19 tests without CEF: 11 in `src/schedule.rs`, 5 in `src/idle.rs` and 3 in `src/lib.rs`.

`src/schedule.rs`:

- A request with `delay = 10 ms` schedules a `Delayed` call at 10 ms. One with 50 ms is capped by a `Periodic` call at 33 ms.
- A new request replaces the pending one. A stream of delayed requests cannot push a call past 33 ms after the last one.
- A call made inside a running call is deferred, counted, and rerun after it. The guard is released when the call unwinds.
- `analyze` accepts the scheduler's calls and flags a periodic call that came too early. One request explains only one call, a request made before the last call started counts as served, and a window keeps the trace before it as history.

Each fix was checked by breaking it on purpose. Removing the reentrancy check fails the reentrancy test. Dropping the delayed path fails 2 tests. Letting a delayed request restart the 33 ms clock, as the first round's pump did, fails the stream test.

`src/idle.rs` tests the frame rule with synthetic paint logs: the r4-style log with one repaint at 4099 ms passes; a second repaint, a full-view repaint at 3.5 s, and a lone repaint at 8 s after an earlier paint-holding repaint before the window each fail, as do a partial repaint, an extra GPUI render and an unexplained pump call. Mutants, applied one at a time (`results/2026-11-23-r5-controls/idle-mutants.sh`, output in `idle-mutants.out`):

| Mutant | Tests that fail |
|---|---|
| Accept two paint-holding repaints (`candidates <= 2`) | 2: the second repaint and the 8 s case |
| `PAINT_HOLDING_TIMEOUT` = 0 s | 3 |
| Allowance counted inside the window only (the second round's rule) | 1: the 8 s case |
| None | 0 of 5 |

`src/lib.rs`: `path_under` matches whole path components, which the isolation scan's allow-list relies on. `report_has_no_template_tokens` fails if REPORT.md still holds a fill token: an uppercase name wrapped in `@` signs, or one wrapped in the same digits on both sides, the shape a broken fill left in commit 7375b80 (`966` before and after each name). Run against that commit's REPORT.md, it fails and lists all of them. A guard in `run-checks.sh` would run before the report is written, and the fill is not a committed step, so the check lives in the test suite.

## Review findings

From the PR #93 verifiers and the OpenCodeReview threads. "Reproduced" means shown on the code before the fix. "By reading" means confirmed in the code but not run. Repro inputs and outputs for the first fix round are in `results/2026-11-23-r3/review-evidence/`; its `run.sh` scripts are marked historical and exit at once.

| Finding | Reproduced? | Fix or reason |
|---|---|---|
| CDP bridge starves the pump | Yes, by the verifier (5 of 5 runs) and controls 1 and 3 here | One bridge timer at a time, lazy pump timers, timeout probe, latency bounds and the timer ledger in `cdp_pass` (see "The CDP slowdown") |
| `idle_pass` rule untested; an 8 s repaint passes after an earlier paint-holding repaint (`scenario.rs:241`) | Yes, the mutants passed all 9 tests of the second round | Rule moved to `src/idle.rs`, allowance counted over the whole navigation, tests and mutants above |
| Runs used the real `HOME` and shared `/tmp`; a sandboxed process mapped the owner's Mesa shader cache | Yes, by the verifier | Scratch `HOME` and `TMPDIR` per run, real `HOME` scanned with an allow-list, control 4 |
| Pump trace only in idle; late fallback calls accepted (`schedule.rs:152`) | Yes: the CDP freeze happened in a scenario the trace did not cover | Trace in every scenario, `pump_pass` fails on a gap over `STEP_TIMEOUT_MS` |
| Request time taken at dequeue (`pump.rs:126`) | By reading | Stamped in `OnScheduleMessagePumpWork`; the pump keeps a timer only while no request is due sooner |
| Trace window dropped a pending request (`pump.rs:80`) | By reading | `analyze_between` keeps the trace before the window as history |
| A request could explain two calls at one `Instant` (`schedule.rs:145`) | Yes, in a unit test | Each call consumes the request |
| Guard stays set after an unwind (`schedule.rs:105`) | Yes, in a unit test | Drop guard |
| Socket tables of the caller's namespace only (`probe.rs:307`) | By reading | Tables read once per network namespace in the tree |
| `build.txt` step fails without git metadata | Yes, by the verifier (`git status` exit 128) | Writes "no git metadata" and continues |
| r3 `review-evidence/zero-window/run.sh` deletes a binary at a fixed path | By reading | All r3 `review-evidence/*/run.sh` marked historical; they exit at once |
| `.gitattributes`: `-whitespace` without `-text` | By reading | Main's `-text -whitespace` rules for `results/**` and `evidence/**` |
| Runner had no scratch `XDG_RUNTIME_DIR`; dconf and the system bus were reached | Yes, negative control above | Scratch runtime dir, private bus on a random socket name in it, `DBUS_SYSTEM_BUS_ADDRESS` to a missing path, `isolation_clean` gates every run, `check-bus-listener.sh` gates the runner |
| Idle repaint cause | Confirmed by the verifier | Documented; idle passes with the paint-holding frame explained |
| 10 ms pump calls unexplained | Confirmed in CEF source | Documented, with the frame-budget impact |
| `requests_delayed` always 0 | Confirmed in Chromium source | Documented; unit tests cover the path |
| Reentrancy guard and delayed path untested | Yes, no caller could reach the guard | Unit tests in `src/schedule.rs`, shown to fail when each fix is removed |
| `run-checks.sh`: Xvfb read can block forever | Yes: with an `Xvfb` that exits at once, the old script hung until killed at 20 s | `read -t` bounded by `STEP_TIMEOUT_MS` |
| `run-checks.sh`: stale report reused after a crash | Yes: app exited 1 and the old script printed the previous report's `pass=true` | Report and PNG removed before each run |
| `run-checks.sh`: exits 0 after failures | Yes | Counts failures and exits 1; the EXIT trap keeps that status |
| `build.rs` checks the host OS | By Cargo semantics; no cross build here | Uses `CARGO_CFG_TARGET_OS` |
| `bridge.rs`: a cancelled call leaks its pending entry | No: every call is awaited to completion, and `detach_all` clears the map | Not changed; the sweeper would also expire such an entry at its deadline |
| `config.rs` accepts a zero-length window | Yes: the old binary passed idle with `--pump-measure-ms=0` | `--pump-measure-ms` and `--pump-step-timeout-ms` must be > 0 |
| `engine.rs` keeps closed browser handles | By reading | `OnBeforeClose` removes the entry and drops the handle |
| `CefShutdown()` called after a browser-close timeout | By reading; not forced at runtime | `CefShutdown()` is skipped and the run fails unless every `OnBeforeClose` arrived |
| `wait_gone` skips malformed child records | Yes: a string PID and an out-of-range PID gave `pass=true` | Each record must parse with a positive `i32` PID |
| `pidfd_open` errors other than ESRCH count as exited | Yes: under `ulimit -n 4` with a live child, the old check gave `pass=true` | Those errors are "inconclusive" and fail the check |
| Pump: delayed requests can postpone the periodic call | Yes, in a unit test with the old rule | The 33 ms deadline counts from the last call and requests cannot move it |
| Listening-socket check passes on unreadable tables | By reading | Errors recorded; `cdp_pass` requires `complete` |
| `view.rs` keeps frames GPUI never drew | Yes by measurement | Undrawn frames are dropped when replaced (`gpui_frames_never_drawn`) |
| `/proc` scan delays the timed DevTools close | Yes, 1 of 15 trials closed 16 ms late in an earlier run | Scan runs only if a timed scan fits before the close; lateness recorded |

### Known limits

The parent agent accepted these without a change. They affect the harness's robustness, not the done checks' results:

- Concurrent invocations for the same CEF archive share one bundle dir, so one can overwrite the binary another is about to start.
- A trial has no external deadline. GPUI's step timers cannot interrupt a synchronous CEF call that hangs (initialization, shutdown), so one hang stops the suite.
- `wait_gone` treats any nonzero pidfd event as exit, so a latent `POLLNVAL` or `POLLERR` would count as gone.
- The runner's `sed` substitutes scratch paths unescaped, so a `TMPDIR` containing `&` or `|` would produce a wrong bus config path.
- Two invocations into one results dir can delete or replace each other's reports.
- `--pump-wait-gone` panics on a malformed report instead of failing cleanly.
- A malformed `/proc/PID/stat` reads as "process exited" in `wait_gone`.
- `pump.rs:80`: a request dequeued just before the idle window could flag its call just after as unexplained. `analyze_between` now keeps the earlier trace, which closes this case too.
- `forbidden_refs` waits for `ss` with no deadline, on the GPUI thread.
- `gpui_frames_never_drawn` counts images built for the view and replaced before GPUI drew them, not frames GPUI skipped in the paint phase. The name overstates it.

Open review threads, not addressed in this round and not classified by the parent:

- `schedule.rs:73`: the 33 ms fallback counts from the previous call's end, not its start, so a 10 ms call puts the next start about 43 ms later. The idle trace shows that maximum.
- `scenario.rs:490`: the 0 ms DevTools close is issued after `ShowDevTools` returns, so no trial closes DevTools while its creation is pending.
- `probe.rs:284`: a readlink failure on an fd entry is skipped without marking the scan incomplete.
- `view.rs:29`: an `OnPaint` that overwrites a frame GPUI has not taken yet is not counted as dropped.

## Cargo 1.100 and cef-rs

Cargo 1.100's default build-dir layout keeps cef-rs's copy of CEF's runtime files out of the binary's directory (tauri-apps/cef-rs#522, open). Workaround: `scripts/run-checks.sh` sources Spike 1's `runtime-paths.sh`, checks the runtime that `prepare.sh` staged (its `archive.json` must match the `pins.toml` beta archive name and SHA-1, and `libcef.so` must be nonempty), and copies it with the binary into `target/release/cef-gpui-pump-bundle/<archive name>`. One bundle per archive keeps files from another CEF build out. The binary has `RPATH=$ORIGIN` (`build.rs`).

The binary's test target is off (`test = false`), because it links libcef and could only start from the bundle. Tests live in the CEF-free library. Cargo then reports the binary's dependencies as unused during `cargo test`, so `unused_dependencies` is allowed for this package.

The archive used is `cef_binary_158.0.3+g8c29be2+chromium-158.0.8127.18_linux64_beta_minimal.tar.bz2`, SHA-1 `057a0b871bf26d26d654d999dba595b92fe0f067` (`pins.toml` `[cef.beta.sha1]`, checked by `prepare.sh`; `results/2026-11-23-r5/cef-archive.json`). The r3 and first-round runs used the stable-channel archive of the same commit, SHA-1 `620b80a02dfe93ed7884a9eb1b265ff34f799024`, which unpatched cef-rs downloads by default.

## Dependencies

- One cef-rs source serves both spikes. `cef-dll-sys` declares `links = "cef_dll_wrapper"`, so Cargo allows only one copy of it in the workspace graph, and a separate git dependency on the same rev cannot sit next to Spike 1's path dependency. This spike takes `cef`, `gpui` and `gpui_platform` from `[workspace.dependencies]` in `rewrite/Cargo.toml`. Those point at `target/spike1-src/{cef-rs,zed}`, which `spikes/gpui-cef-compositor/prepare.sh` downloads at the `pins.toml` revs and patches. Each staged tree has a `.flecha-revision` stamp, and the patched `cef-dll-sys` build script fails unless both stamps equal the `pins.toml` repository and rev (cef-rs `Timpan4/cef-rs` `32d6bc55`, Zed `f126950f`). `Cargo.lock` has a single `cef-dll-sys` entry, a path package.
- The cef-rs patch changes CEF discovery in `sys/build.rs`, adds a `FLECHA_CEF_MINIMAL_SHA1` constant, and edits `osr_texture_import`, which is behind `accelerated_osr` and not built here. The Zed patch touches `gpui_linux`'s X11 and Wayland windows and `gpui_wgpu`, which this spike runs on.
- The workspace `cef` has no `accelerated_osr`, so cef-rs's optional registry `wgpu = "30"` is not in the graph. `Cargo.lock` has one wgpu, Zed's git rev `cddb2f5d`.
- The workspace `gpui_platform` enables `font-kit`, `x11` and `wayland`; the last two turn on `gpui_linux`'s display backends. `gpui` has `default-features = false`. On Linux its default features add nothing this spike needs (`font-kit` and `wayland` are empty, `x11` only reaches the optional `scap`).
- `[patch.crates-io]` in `rewrite/Cargo.toml` is main's: `calloop` at `zed-industries/calloop` `3759371f`, the same entry as Zed's `Cargo.toml` at `f126950f`. Zed at that rev also patches `async-task` (`smol-rs` `41fd3359`, a waker identity fix), `async-process` (`zed-industries` `0b6d6713`) and `khronos-egl` (`zed-industries` `42214b36`, no panic when a failed EGL call reports success). r3 applied those three, and this workspace does not: they resolve to crates.io `async-task 4.7.1`, `async-process 2.5.0` and `khronos-egl 6.0.0`. Adding them would replace entries in main's `Cargo.lock`, not only add this spike's.
- Merging main at `04b013d` (Spike 0) left main's `Cargo.toml` members and lock in place. Against main, this branch's `Cargo.lock` only adds packages: `cef-gpui-pump`, `env_filter`, `env_logger`, `jiff`, `jiff-core` and `jiff-static`.

## Differences between runs

r4 differed from r3 in its build: Spike 1's patched cef-rs and Zed instead of unpatched git dependencies on the same revs, the beta archive instead of the stable archive of the same CEF commit, and `async-task`, `async-process` and `khronos-egl` from crates.io instead of Zed's forks.

r5 differs from r4 in code, not in resolved dependencies: the bridge sweeper, lazy pump timers, request stamps at enqueue, the pump trace in every scenario, and scratch `HOME` and `TMPDIR`. The merge of main at `04b013d` added main's Spike 0 packages to `Cargo.lock`. The only lines it removed are bare dependency names that became version-qualified, so no package this spike already had changed version. Every pass condition held in both runs. Idle pump calls went from 1,066 to 1,188: periodic calls stayed at 901 and 902, and immediate requests rose from 165 to 286. CDP evaluate calls in the 30 s loop went from 540,555 to 786,719, with p50 latency down from 0.038 to 0.029 ms. r4 armed a GPUI timer for every call. GPUI render rates in the animation runs matched r4 within 0.1 frames/s. There is one run per build, so this report does not attribute the rise in idle requests to one change.

## Not verified

- Wayland, real X11 with a window manager, macOS and Windows. Everything ran on Xvfb.
- GPUI frame pacing on a focused window. Under Xvfb the window stays inactive.
- The pump's `delay > 0` path and reentrancy guard in the running app. Only the unit tests exercise them, because CEF sends no delayed requests here and nothing can re-enter the pump task.
- `CefShutdown()` being skipped after a browser-close timeout. No run timed out, and there is no hook to force one.
- Connections that open and close between the two isolation samples, and renderer sockets' peers, which `ss` cannot resolve from the browser's network namespace.
- That the #4353 check fails on a build without the fix.
- Why the per-call timers stalled some runs and not others, including r4's.
- Input, IME, resize and DPI changes. They are not in this spike's done check.
