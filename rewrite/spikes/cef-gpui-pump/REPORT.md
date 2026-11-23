# Spike 2: CEF message pump in GPUI, and per-browser DevTools access

Date: 2026-11-23. Pins from `rewrite/pins.toml`: CEF `158.0.3+g8c29be2+chromium-158.0.8127.18`, cef-rs `32d6bc55` (Timpan4/cef-rs), Zed/GPUI `f126950f`.

Current results: `results/2026-11-23-r3/`, one run of `scripts/run-checks.sh` as committed (see "How it was run"). `results/2026-11-23/` holds the first round, from before the review fixes. It is kept for history, and this report cites it only where it says so.

## Verdict

| Done check | Result |
|---|---|
| Pump runs only on CEF requests or CEF's documented periodic wake-up | Pass. 0 unexplained calls in @IDLE_CALLS@ calls over 30 s |
| Static page: GPUI draws no frames | Pass, with one explained frame. GPUI drew 1 frame for CEF's one paint-holding repaint at @HOLD_MS@ ms after navigation, and nothing else (see "Pump and idle") |
| Idle CPU (measurement only) | @IDLE_CPU@ % of one core for the whole process tree |
| Page and GPUI animate together | Pass. Both frame counters advance |
| `navigator.webdriver === false` during CDP calls | Pass. @CDP_CALLS@ calls in 30.0 s, @CDP_SAMPLES@ page samples, 0 true. A control run with `--enable-automation` fails |
| CDP without a port or pipe | Pass. No `remote-debugging` switch, no listening TCP socket, every fd and socket table read |
| DevTools close does not kill OSR renderers (CEF #4353) | Pass in 15 of 15 fresh processes. No workaround needed |
| Clean shutdown | Pass. `CefShutdown()` returns in @SHUT_RANGE@ ms. Every child exited in all 19 runs |
| Runs isolated from the desktop session | Pass in 19 of 19 runs. A control run with the first round's environment fails |

## What the spike is

`cef-gpui-pump` is one binary. It is the CEF browser process and every CEF subprocess, and it opens a GPUI window (`gpui_platform::application()`). The window shows one windowless, so Alloy-style, CEF browser. Its software `OnPaint` BGRA buffer becomes a GPUI `RenderImage`. Images GPUI drew are released with `Window::drop_image` once replaced. Frames replaced before GPUI drew them are dropped at once.

- `src/schedule.rs` (library, unit-tested, no CEF) decides when to call `CefDoMessageLoopWork()`. It calls right away for `delay <= 0`, after the delay for `delay > 0`, and no later than 33 ms after the previous call ended. A new request replaces the pending one, as `OnScheduleMessagePumpWork` specifies, but cannot push the 33 ms deadline back. The 33 ms is from CEF 158 `include/cef_app.h`: "A maximum interval of approximately 33 milliseconds between calls (30 calls per second) is recommended." A guard defers a call made while one is running until it returns, as `cef_app.h` requires.
- `src/pump.rs` wires that to CEF and GPUI. `OnScheduleMessagePumpWork` only sends the delay on a channel. A GPUI main-thread task owns the schedule, makes the guarded call, and logs each call with its reason. `schedule::analyze` checks the log.
- `src/bridge.rs` is the in-process agent bridge. It sends CDP messages with `CefBrowserHost::SendDevToolsMessage` and reads replies with an `AddDevToolsMessageObserver` registration per browser. It lists tabs (`Target.getTargetInfo`), evaluates JavaScript (`Runtime.evaluate`) and takes screenshots (`Page.captureScreenshot`). Every call has a timeout, because CEF drops malformed messages without a reply.
- `src/scenario.rs` holds the scripted checks. `src/probe.rs` reads `/proc` for the process tree, CPU time, command lines, sockets and references to the desktop session.

## How it was run

```sh
cd rewrite
flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
  bash ~/.claude/skills/hosts/scripts/agent-build run cargo build --release -j 14 -p cef-gpui-pump
flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
  bash ~/.claude/skills/hosts/scripts/agent-build run cargo test --release -j 14 -p cef-gpui-pump
cd spikes/cef-gpui-pump
WARMUP_MS=3000 MEASURE_MS=30000 STEP_TIMEOUT_MS=20000 DEVTOOLS_CLOSE_MS="0 100 2000" DEVTOOLS_TRIALS=5 \
  scripts/run-checks.sh results/2026-11-23-r3
```

The runner exits 0 only if every run passes. It ran as a transient `systemd-run --user` unit and printed `all runs passed` (`results/2026-11-23-r3/run.log`). The timing values are harness parameters, not product values. `STEP_TIMEOUT_MS` bounds each wait for CEF (Xvfb start, load, a CDP reply, DevTools open and close, browser close, child exit). The runner stages a bundle in `target/release/cef-gpui-pump-bundle` (see "Cargo 1.100 and cef-rs"). Every run writes a JSON report, its stderr, and a listing of its scratch runtime dir.

Build: release profile. GPUI picked `NVIDIA GeForce RTX 5080 (Vulkan)` on Xvfb (`RUST_LOG=gpui_wgpu=info`, first-round smoke run), so GPUI was not on a software renderer. CEF itself used software OSR (`OnPaint`, no shared textures). Chromium switches: `--ozone-platform=x11 --password-store=basic`. The sandbox was on.

### Isolation

Each app run gets:

- Xvfb (`-nolisten tcp`) on a display it picks, with `WAYLAND_DISPLAY` and `WAYLAND_SOCKET` unset.
- A fresh `mktemp -d` CEF cache and a fresh `mktemp -d` `XDG_RUNTIME_DIR` (mode 700), both deleted afterwards.
- `dbus-run-session` with `scripts/isolated-session-bus.conf`: no service directories, so nothing is activated, and the bus listens at `$XDG_RUNTIME_DIR/bus` in the scratch dir. That config must only run with a scratch runtime dir. A test with two buses on one scratch dir showed dbus-daemon replacing an existing socket at its path and deleting it on exit, so with the desktop's runtime dir it would remove `/run/user/1000/bus`. The runner always passes a new `mktemp -d` dir.
- `DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent`.

The app checks this itself. After the page loads and again before shutdown, `probe::forbidden_refs` lists every file the process tree has open or memory-mapped and every Unix socket peer (from `ss -xan`), and fails the run if any lies under the caller's `XDG_RUNTIME_DIR` (`/run/user/1000`) or `/run/dbus/system_bus_socket`. In r3 all 38 samples were clean, with every fd table and map readable. The limits: it samples twice, so a connection opened and closed between samples is missed, and sandboxed renderers live in their own network namespace, so their sockets (@NO_PEER@ per sample) get no peer path from `ss`. Their open and mapped files are still scanned.

What used to reach the desktop session now lands in the scratch dir. Every run's listing shows `dconf/user` there (`*.runtime-dir.txt`).

Negative control: one manual idle run with the first round's environment (the caller's `XDG_RUNTIME_DIR`, no system bus override, the old bus config listening in `/tmp`; `results/2026-11-23-r3/review-evidence/isolation-control/`). It failed `isolation_clean`. The browser process had `/run/user/1000/dconf/user` and `/run/user/1000/dconf/cosmic` mapped and 4 connections to `/run/dbus/system_bus_socket`.

The first round's very first smoke run used stock `dbus-run-session`, which activated a private gvfs daemon, xdg-desktop-portal, the document portal, the gtk and cosmic portal backends and the a11y bus. gvfs failed to mount `/run/user/1000/gvfs` (permission denied). No process from that run survives, and the desktop session's gvfs mount is intact.

Related host state, not caused by this spike as far as the journal shows: the desktop session's `xdg-document-portal.service` exited with status 21 at 08:41:56 and stayed failed, so `/run/user/1000/doc` was not mounted. The first smoke run's Xvfb started at 08:42:00, after that exit. The portal processes logging in that second (`gnome-keyring-daemon` 23676, `xdg-desktop-portal-cosmic` 23705) ran in another agent session's systemd scope. The cause is undetermined. With the owner's approval the service was restarted. At 10:41:30 CET it became active with exit status 0, and `/run/user/1000/doc` is mounted again (`fuse.portal`).

No test window opened on the desktop session, and the owner's Flecha install and profiles were not touched.

## Pump and idle

Static page `pages/static.html`: text only, no animation, timers, focusable input or caret. `results/2026-11-23-r3/idle.json`, 3 s warmup after load, 30 s window:

| Measure | Value |
|---|---|
| Pump calls | @IDLE_CALLS@: @IDLE_PERIODIC@ periodic, @IDLE_IMM@ for immediate requests, 0 delayed, 0 rerun |
| CEF requests | @IDLE_IMM@, all `delay_ms <= 0` |
| Unexplained calls | 0 (each periodic call came at least 33 ms after the previous call ended) |
| Gap between calls | p50 @GAP50@ ms, p99 @GAP99@ ms, max @GAPMAX@ ms |
| Time inside `CefDoMessageLoopWork` | @WORK_TOTAL@ ms total, p50 @WORK50@ ms, p99 @WORK99@ ms, max @WORKMAX@ ms |
| CPU, whole tree | @IDLE_CPU@ % of one core (@IDLE_RUN_MS@ ms run time in 30 s) |
| GPUI renders / OnPaint in the window | 1 / 1, the paint-holding repaint |

CPU method: sum over the process tree of `/proc/PID/task/*/schedstat` run time, between snapshots at the window's edges. `utime + stime` ticks from `/proc/PID/stat` are in the report too.

### The one frame: paint holding

The one paint is a full-view repaint (`[0, 0, 1024, 640]`) @HOLD_MS@ ms after navigation. The first round saw it in 4 of 4 runs with a 3 s warmup, 3995 to 4007 ms after the first paint, and never with a 6 s warmup.

It is Chromium's paint-holding timeout. In the local Chromium 158.0.8119.0 tree, `third_party/blink/common/widget/constants.cc:15` sets `kNewContentRenderingDelay = base::Seconds(4)`. `RenderWidgetHostImpl::InitializePaintHolding` starts it at navigation, and on expiry `ClearDisplayedGraphics` makes the renderer repaint once. The PR verifier confirmed the cause: a CDP `Input.dispatchMouseEvent` about 1.5 s after load moved the repaint to 1503 ms and removed the 4 s paint. That path is `content/browser/devtools/protocol/input_handler.cc:694`, which calls `ForceFirstFrameAfterNavigationTimeout()`.

The parent agent's decision for this fix round: the idle check passes with that frame explained. `idle_pass` therefore requires 0 unexplained pump calls, no more GPUI renders than CEF paints in the window, and every paint in the window to be that repaint: at most one, of the whole view, at least `kNewContentRenderingDelay` after navigation. `idle_frames.strict_no_frames` records whether the window had no frame at all.

### 10 ms calls: CEF's pump time slice

The slowest `CefDoMessageLoopWork()` calls take 10.00 ms (@WORKMAX@ ms max in r3). That is CEF's external pump time slice. CEF 8c29be2 `libcef/browser/browser_message_loop.cc:107` creates `MessagePumpExternal(0.01f, handler)`, and its `Run()` (lines 29 to 49) keeps running work until the next task is more than `max_time_slice_` (10 ms) past the call's start. When a delayed task is due within 10 ms, the call waits for it. So one pump call can hold GPUI's main thread for up to 10 ms, which is most of a 16.7 ms frame at 60 Hz. Phase 1's frame budget has to allow for it.

### Why CEF never asks for delayed work

`requests_delayed` is 0 in every run, by design. Chromium's `ThreadControllerWithMessagePumpImpl::SetNextDelayedDoWork` (`base/task/sequence_manager/thread_controller_with_message_pump_impl.cc:199` to `224`) only schedules delayed work when the thread is not inside `DoWork`, and nearly all delayed tasks are posted from inside it. CEF timers therefore rely on the 33 ms periodic call. The `delay > 0` path and the reentrancy guard are covered by unit tests (see "Unit tests").

## Page and GPUI animation together

Page `pages/anim.html`: a CSS `@keyframes` box and a `requestAnimationFrame` counter. Page counters were read over CDP at the window's edges. Each phase: 3 s warmup, 30 s window.

| Run | GPUI animation | Page rAF/s | CSS time advanced | OnPaint/s | GPUI renders/s | GPUI animation steps | CPU, tree |
|---|---|---|---|---|---|---|---|
@ANIM_ROWS@

The page holds 30 frames/s in every case. `include/internal/cef_types.h` gives `windowless_frame_rate` "the default value is 30", and the spike does not change it. With default window options, GPUI draws about 21 frames/s. The window is never focused under Xvfb with no window manager, so GPUI's `inactive_frame_interval` (default 33.3 ms, "approximately 30 fps") applies. With `inactive_frame_interval: None` (`--pump-no-inactive-throttle`), GPUI shows every page frame, and draws more while its own animation runs. Its CPU numbers include those extra frames. On a focused window on a real session, the default throttle should not apply. That is unverified here.

## Agent access through `SendDevToolsMessage`

`results/2026-11-23-r3/cdp.json`:

- List tabs: 1 tab, `Target.getTargetInfo` returned `type: page`, title `anim`, the file URL.
- Evaluate: `Runtime.evaluate("navigator.webdriver")` back to back. The loop itself ran @CDP_LOOP_S@ s for a 30 s window: @CDP_CALLS@ calls, every reply `false`, no errors. Per-call latency: p50 @LAT50@ ms, p99 @LAT99@ ms, max @LATMAX@ ms. The page counted `navigator.webdriver` on every animation frame: @CDP_SAMPLES@ samples during the loop, 0 not `false`, and its rAF rate over the loop was @CDP_RAF@ frames/s.
- Screenshot: `Page.captureScreenshot` returned a 1024 × 640 PNG (`cdp.png`) in @SHOT_MS@ ms.
- Correction to the first round: its report quoted 30.13 s, which was measured after the screenshot and the `/proc` scans, not the loop alone. The PR verifier reported 47.3, 33.4 and 34.7 s from that same field, and 903, 817 and 818 page frames. Only the frame counts show the page slowing down, and this round did not reproduce it: the loop ran 30.000 s and 30.006 s in two full runs, at 30 page frames/s. The loop's length, per-call latency and page frame rate are now recorded separately, before the screenshot. In r3 the loop ended within one call of 30 s, and the page kept 30 frames/s.
- Negative control (`results/2026-11-23/cdp-enable-automation/`, first-round build): the same check with `EXTRA_ARGS=--enable-automation`, 3 s warmup and a 5 s window. All 58,080 replies and all 153 page samples were `true`, and the run failed. The check detects `navigator.webdriver` turning on.
- No port or pipe: no process in the tree has `remote-debugging` or `enable-automation` on its command line. No process in the tree has a listening TCP socket. The run now fails unless every fd table and socket table was read (`listening_sockets.complete`), and in r3 all were. The one listening socket is Chromium's ProcessSingleton Unix socket (`/tmp/org.chromium.Chromium.*/SingletonSocket`), which is not a DevTools endpoint.

## DevTools and CEF #4353

CEF #4353: with every browser windowless and the app on `CefDoMessageLoopWork()`, closing a DevTools window started browser shutdown and killed renderers. The issue's own results: a first close 0 to 200 ms after `ShowDevTools` froze every page, and a close at 1 to 3 s froze the uninspected page. CEF fixed it in `11d50773` ("acquire an application keep-alive on the first CefDoMessageLoopWork call"). The pinned build `8c29be2` is 6 commits ahead of `11d50773` and 0 behind (GitHub compare).

Test: 2 windowless browsers on the animation page. Page 0 is inspected, page 1 is not. `ShowDevTools` gets a default (windowed) `CefWindowInfo` and a real client, `CloseDevTools()` runs N ms after the `ShowDevTools` call, and then each page is checked: `OnPaint` rate over 30 s, `Runtime.evaluate("1 + 1")`, renderer processes, `OnRenderProcessTerminated`. 5 fresh processes per delay.

| Close delay | Close after `ShowDevTools` | `ShowDevTools` call | Pages' OnPaint/s after | Evaluate after | Renderer terminations |
|---|---|---|---|---|---|
@DT_ROWS@

`ShowDevTools()` is synchronous: it returns after creating the DevTools browser, and that took up to @SHOW_MAX@ ms. A close requested sooner than that runs when the call returns, so the 0 ms and 100 ms delays land later than asked. The DevTools browser was `CEF_RUNTIME_STYLE_CHROME` in its own X11 window. The `/proc` scan while DevTools is open now runs only if a scan as long as the one timed just before fits before the close. @DT_SCAN_NOTE@

Result: #4353 does not reproduce on CEF 158.0.3, and no workaround is needed. The check was not run against a build without the fix (for example CEF 156), so its ability to catch the freeze is not shown here.

## Shutdown

Each run closes every browser with `CloseBrowser(true)` and waits for every `OnBeforeClose`, which removes that browser's handle. Then it drops the observer registrations, quits GPUI, and calls `CefShutdown()`. If any browser is still open when the wait times out, the app skips `CefShutdown()` (calling it with live browsers is not allowed), records why, and fails the run. `CefShutdown()` returned in @SHUT_RANGE@ ms. Right after it returns, some zygote processes are still alive. They exit when the browser process exits and closes their sockets. So after the app exits, the runner calls `cef-gpui-pump --pump-wait-gone=REPORT`. It waits on a pidfd for each child the app saw before shutdown, bounded by `STEP_TIMEOUT_MS`. A malformed child record, or a `/proc` or `pidfd_open` error other than "no such process", fails the check instead of counting as gone. In all 19 runs, every child had exited before that wait began.

Process types come from `--type=`, but zygote-forked processes keep `--type=zygote` on this build. `probe::Proc::kind` labels them by tree shape. The sandboxed layout seen here is browser, then the sandboxed zygote (PID-namespace init), then the zygote worker, then one process per renderer, each in its own PID namespace.

## Unit tests

`cargo test --release -p cef-gpui-pump` runs 7 tests in `src/schedule.rs`, without CEF:

- A request with `delay = 10 ms` schedules a `Delayed` call at 10 ms. One with 50 ms is capped by a `Periodic` call at 33 ms.
- A new request replaces the pending one.
- A stream of delayed requests cannot push a call past 33 ms after the last one.
- A call made inside a running call is deferred, counted, and rerun after it.
- `analyze` accepts the scheduler's calls and flags a periodic call that came too early.

Each fix was checked by breaking it on purpose. Removing the reentrancy check fails the reentrancy test. Dropping the delayed path fails 2 tests. Letting a delayed request restart the 33 ms clock, as the first round's pump did, fails the stream test.

## Review findings

From the PR #93 verifier and 15 OpenCodeReview leads. "Reproduced" means shown on the first-round code (commit d7ade4a) before the fix. "By reading" means confirmed in the code but not run. Repro inputs and outputs are in `results/2026-11-23-r3/review-evidence/`.

| Finding | Reproduced? | Fix or reason |
|---|---|---|
| Runner had no scratch `XDG_RUNTIME_DIR`; dconf and the system bus were reached | Yes, negative control above | Scratch runtime dir, bus at `$XDG_RUNTIME_DIR/bus`, `DBUS_SYSTEM_BUS_ADDRESS` to a missing path, `isolation_clean` gates every run |
| Idle repaint cause | Confirmed by the verifier | Documented; idle passes with the paint-holding frame explained |
| 10 ms pump calls unexplained | Confirmed in CEF source | Documented, with the frame-budget impact |
| `requests_delayed` always 0 | Confirmed in Chromium source | Documented; unit tests cover the path |
| CDP loop length and page rate misreported | Reported by the verifier; the slowdown did not reproduce here (30.0 s loops at 30 frames/s) | Loop length, per-call latency and page rAF rate recorded before the screenshot; report corrected |
| Reentrancy guard and delayed path untested | Yes, no caller could reach the guard | Unit tests in `src/schedule.rs`, shown to fail when each fix is removed |
| `run-checks.sh`: Xvfb read can block forever | Yes: with an `Xvfb` that exits at once, the old script hung until killed at 20 s (`review-evidence/runner/`) | `read -t` bounded by `STEP_TIMEOUT_MS`; new script exits 1 after 3 s |
| `run-checks.sh`: stale report reused after a crash | Yes: app exited 1 and the old script printed the previous report's `pass=true` (`review-evidence/runner/`) | Report and PNG removed before each run |
| `run-checks.sh`: exits 0 after failures | Yes, same run exited 0 | Counts failures and exits 1; the EXIT trap keeps that status |
| `build.rs` checks the host OS | By Cargo semantics (build scripts are compiled for the host); no cross build here | Uses `CARGO_CFG_TARGET_OS` |
| `bridge.rs`: a cancelled call leaks its pending sender | No: every call is awaited to completion, and `detach_all` clears the map | Not changed |
| `config.rs` accepts a zero-length window | Yes: the old binary passed idle with `--pump-measure-ms=0`, a 19 ms window and 0 pump calls (`review-evidence/zero-window/`) | `--pump-measure-ms` and `--pump-step-timeout-ms` must be > 0 |
| `engine.rs` keeps closed browser handles | Yes by reading: handles stayed in `tracked` until teardown | `OnBeforeClose` removes the entry and drops the handle after the borrow |
| `CefShutdown()` called after a browser-close timeout | Yes by reading; not forced at runtime | `CefShutdown()` is skipped and the run fails unless every `OnBeforeClose` arrived |
| `wait_gone` skips malformed child records | Yes: a string PID and an out-of-range PID gave `pass=true` (`review-evidence/wait-gone/`) | Each record must parse with a positive `i32` PID |
| `pidfd_open` errors other than ESRCH count as exited | Yes: under `ulimit -n 4` with a live child, the old check gave `pass=true` (`review-evidence/wait-gone/`). The `/proc` read after `pidfd_open` had the same hole | Those errors are "inconclusive" and fail the check |
| Pump: delayed requests can postpone the periodic call | Yes, in a unit test with the old rule | The 33 ms deadline counts from the last call and requests cannot move it |
| Listening-socket check passes on unreadable tables | Yes by reading: table read errors were ignored | Errors recorded; `cdp_pass` requires `complete` |
| `view.rs` keeps frames GPUI never drew | Yes by measurement: frames arrive at 30/s and GPUI draws about 21/s, so about 9/s waited for the next draw | Undrawn frames are dropped when replaced (`gpui_frames_never_drawn`) |
| `/proc` scan delays the timed DevTools close | Yes, in an earlier full run of this round (output not committed): 1 of 15 trials closed 16 ms late because of it | Scan runs only if a timed scan fits before the close; lateness recorded |

## Cargo 1.100 and cef-rs

Cargo 1.100's default build-dir layout puts `OUT_DIR` at `target/release/build/cef-dll-sys/<hash>/out`. cef-dll-sys copies CEF's runtime files to the third parent of that, `target/release/build`, not next to the binary (tauri-apps/cef-rs#522, open). Workaround: `scripts/run-checks.sh` builds `target/release/cef-gpui-pump-bundle` from the binary and the CEF distribution in cef-dll-sys's `OUT_DIR`, whose `archive.json` must name the pinned version. The binary has `RPATH=$ORIGIN` (`build.rs`).

The binary's test target is off (`test = false`), because it links libcef and could only start from the bundle. Tests live in the CEF-free library. Cargo then reports the binary's dependencies as unused during `cargo test`, so `unused_dependencies` is allowed for this package.

The archive used is `cef_binary_158.0.3+g8c29be2+chromium-158.0.8127.18_linux64_minimal.tar.bz2`, SHA-1 `620b80a02dfe93ed7884a9eb1b265ff34f799024`. That is the stable-channel archive of the pinned commit (`pins.toml` `[cef.stable.sha1]`), as the plan's beta archive gap says.

## Dependencies

- cef-rs without `accelerated_osr`: its registry `wgpu = "30"` is optional and not in the graph. `Cargo.lock` has one wgpu, Zed's git rev `cddb2f5d`. No conflict for this spike.
- `gpui_platform` needs `features = ["x11", "wayland"]`. Zed's workspace turns off `gpui_linux`'s default backends, and without the features `gpui_linux` builds with no display backend.
- `rewrite/Cargo.toml` copies Zed's `[patch.crates-io]` entries for crates GPUI uses (`async-task`, `async-process`, `calloop`, `khronos-egl`). Zed's patches do not apply to git dependents.

## Not verified

- Wayland, real X11 with a window manager, macOS and Windows. Everything ran on Xvfb.
- GPUI frame pacing on a focused window. Under Xvfb the window stays inactive.
- The pump's `delay > 0` path and reentrancy guard in the running app. Only the unit tests exercise them, because CEF sends no delayed requests here and nothing can re-enter the pump task.
- `CefShutdown()` being skipped after a browser-close timeout. No run timed out, and there is no hook to force one.
- Connections that open and close between the two isolation samples, and renderer sockets, which `ss` cannot resolve from the browser's network namespace.
- That the #4353 check fails on a build without the fix.
- Input, IME, resize and DPI changes. They are not in this spike's done check.
- The plan's beta archive. The build used the stable archive of the same commit.
