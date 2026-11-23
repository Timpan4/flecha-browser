# Spike 2: CEF message pump in GPUI, and per-browser DevTools access

Date: 2026-11-23. Pins from `rewrite/pins.toml`: CEF `158.0.3+g8c29be2+chromium-158.0.8127.18` (beta-channel minimal archive), cef-rs `32d6bc55` (Timpan4/cef-rs), Zed/GPUI `f126950f`. cef-rs and Zed are the sources Spike 1's `prepare.sh` stages and patches (see "Dependencies").

Current results: `results/2026-11-23-r4/`, one run of `scripts/run-checks.sh` as committed, on the head that merges Spike 1 (see "How it was run"). `build.txt` there names the commit and `cef-archive.json` the CEF archive. History, cited only where this report says so: `results/2026-11-23/` is the first round, before the review fixes. `results/2026-11-23-r3/` is the first fix round. It used a bus listener that could replace an existing socket (see "Isolation"), and the pre-merge build (see "Differences from r3"). Its `review-evidence/` holds the repro inputs and outputs.

## Verdict

| Done check | Result |
|---|---|
| Pump runs only on CEF requests or CEF's documented periodic wake-up | Pass. 0 unexplained calls in 1066 calls over 30 s |
| Static page: GPUI draws no frames | Pass, with one explained frame. GPUI drew 1 frame for CEF's one paint-holding repaint at 4099 ms after navigation, and nothing else (see "Pump and idle") |
| Idle CPU (measurement only) | 0.63 % of one core for the whole process tree |
| Page and GPUI animate together | Pass. Both frame counters advance |
| `navigator.webdriver === false` during CDP calls | Pass. 540,555 calls in 30.0 s, 900 page samples, 0 true. A control run with `--enable-automation` fails |
| CDP without a port or pipe | Pass. No `remote-debugging` switch, no listening TCP socket, every fd and socket table read |
| DevTools close does not kill OSR renderers (CEF #4353) | Pass in 15 of 15 fresh processes. No workaround needed |
| Clean shutdown | Pass. `CefShutdown()` returns in 48 to 122 ms. Every child exited in all 19 runs |
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
bash spikes/gpui-cef-compositor/prepare.sh linux64
CARGO_NET_GIT_FETCH_WITH_CLI=true flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
  bash ~/.claude/skills/hosts/scripts/agent-build run cargo build --locked --release -j 14 -p cef-gpui-pump
flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
  bash ~/.claude/skills/hosts/scripts/agent-build run cargo test --locked --release -j 14 -p cef-gpui-pump
cd spikes/cef-gpui-pump
WARMUP_MS=3000 MEASURE_MS=30000 STEP_TIMEOUT_MS=20000 DEVTOOLS_CLOSE_MS="0 100 2000" DEVTOOLS_TRIALS=5 \
  scripts/run-checks.sh results/2026-11-23-r4
```

The runner exits 0 only if every run passes. It ran as a transient `systemd-run --user` unit and printed `all runs passed` (`results/2026-11-23-r4/run.log`). The timing values are harness parameters, not product values. `STEP_TIMEOUT_MS` bounds each wait for CEF (Xvfb start, load, a CDP reply, DevTools open and close, browser close, child exit). The runner stages a bundle in `target/release/cef-gpui-pump-bundle/<archive name>` (see "Cargo 1.100 and cef-rs"). Every run writes a JSON report, its stderr, and a listing of its scratch runtime dir.

Build: release profile. GPUI picked `NVIDIA GeForce RTX 5080 (Vulkan)` on Xvfb (`RUST_LOG=gpui_wgpu=info`, first-round smoke run), so GPUI was not on a software renderer. CEF itself used software OSR (`OnPaint`, no shared textures). Chromium switches: `--ozone-platform=x11 --password-store=basic`. The sandbox was on.

### Isolation

Each app run gets:

- Xvfb (`-nolisten tcp`) on a display it picks, with `WAYLAND_DISPLAY` and `WAYLAND_SOCKET` unset.
- A fresh `mktemp -d` CEF cache and a fresh `mktemp -d` `XDG_RUNTIME_DIR` (mode 700), both deleted afterwards.
- `dbus-run-session` with a config written from `scripts/isolated-session-bus.conf.in`: no service directories, so nothing is activated, and `<listen>unix:dir=SCRATCH</listen>`, so dbus-daemon creates a socket with a new random name in that run's scratch dir. The first fix round listened at `$XDG_RUNTIME_DIR/bus` (`unix:runtime=yes`). A test with two buses on one scratch dir showed that listener replacing an existing socket at its path and deleting it on exit, so pointed at the desktop's runtime dir it would remove `/run/user/1000/bus`. `scripts/check-bus-listener.sh` starts a second bus on the same dir while the first runs and requires the first socket to be the same file and still answering afterwards. The runner runs it before any app run. It passes with the new listener and fails with the old one ("second bus reused …/bus", "first bus socket … is gone").
- `DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent`.

The app checks this itself. After the page loads and again before shutdown, `probe::forbidden_refs` lists every file the process tree has open or memory-mapped and every Unix socket peer (from `ss -xan`), and fails the run if any lies under the caller's `XDG_RUNTIME_DIR` (`/run/user/1000`) or `/run/dbus/system_bus_socket`. In r4 all 38 samples were clean, with every fd table and map readable and no `ss` error. The limits: it samples twice, so a connection opened and closed between samples is missed, and sandboxed renderers live in their own network namespace, so their sockets (13 to 22 per sample) get no peer path from `ss`. Their open and mapped files are still scanned.

What used to reach the desktop session now lands in the scratch dir. Every run's listing shows `dconf/user` there (`*.runtime-dir.txt`).

Negative control: one manual idle run with the first round's environment (the caller's `XDG_RUNTIME_DIR`, no system bus override, the old bus config listening in `/tmp`; `results/2026-11-23-r3/review-evidence/isolation-control/`). It failed `isolation_clean`. The browser process had `/run/user/1000/dconf/user` and `/run/user/1000/dconf/cosmic` mapped and 4 connections to `/run/dbus/system_bus_socket`.

The first round's very first smoke run used stock `dbus-run-session`, which activated a private gvfs daemon, xdg-desktop-portal, the document portal, the gtk and cosmic portal backends and the a11y bus. gvfs failed to mount `/run/user/1000/gvfs` (permission denied). No process from that run survives, and the desktop session's gvfs mount is intact.

Related host state, not caused by this spike as far as the journal shows: the desktop session's `xdg-document-portal.service` exited with status 21 at 08:41:56 and stayed failed, so `/run/user/1000/doc` was not mounted. The first smoke run's Xvfb started at 08:42:00, after that exit. The portal processes logging in that second (`gnome-keyring-daemon` 23676, `xdg-desktop-portal-cosmic` 23705) ran in another agent session's systemd scope. The cause is undetermined. With the owner's approval the service was restarted. At 10:41:30 CET it became active with exit status 0, and `/run/user/1000/doc` is mounted again (`fuse.portal`).

No test window opened on the desktop session, and the owner's Flecha install and profiles were not touched.

## Pump and idle

Static page `pages/static.html`: text only, no animation, timers, focusable input or caret. `results/2026-11-23-r4/idle.json`, 3 s warmup after load, 30 s window:

| Measure | Value |
|---|---|
| Pump calls | 1066: 901 periodic, 165 for immediate requests, 0 delayed, 0 rerun |
| CEF requests | 165, all `delay_ms <= 0` |
| Unexplained calls | 0 (each periodic call came at least 33 ms after the previous call ended) |
| Gap between calls | p50 33.04 ms, p99 33.07 ms, max 43.06 ms |
| Time inside `CefDoMessageLoopWork` | 87.9 ms total, p50 0.004 ms, p99 0.22 ms, max 10.003 ms |
| CPU, whole tree | 0.63 % of one core (189 ms run time in 30 s) |
| GPUI renders / OnPaint in the window | 1 / 1, the paint-holding repaint |

CPU method: sum over the process tree of `/proc/PID/task/*/schedstat` run time, between snapshots at the window's edges. `utime + stime` ticks from `/proc/PID/stat` are in the report too.

### The one frame: paint holding

The one paint is a full-view repaint (`[0, 0, 1024, 640]`) 4099 ms after navigation. The first round saw it in 4 of 4 runs with a 3 s warmup, 3995 to 4007 ms after the first paint, and never with a 6 s warmup.

It is Chromium's paint-holding timeout. In the local Chromium 158.0.8119.0 tree, `third_party/blink/common/widget/constants.cc:15` sets `kNewContentRenderingDelay = base::Seconds(4)`. `RenderWidgetHostImpl::InitializePaintHolding` starts it at navigation, and on expiry `ClearDisplayedGraphics` makes the renderer repaint once. The PR verifier confirmed the cause: a CDP `Input.dispatchMouseEvent` about 1.5 s after load moved the repaint to 1503 ms and removed the 4 s paint. That path is `content/browser/devtools/protocol/input_handler.cc:694`, which calls `ForceFirstFrameAfterNavigationTimeout()`.

The parent agent's decision for this fix round: the idle check passes with that frame explained. `idle_pass` therefore requires 0 unexplained pump calls, no more GPUI renders than CEF paints in the window, and every paint in the window to be that repaint: at most one, of the whole view, at least `kNewContentRenderingDelay` after navigation. `idle_frames.strict_no_frames` records whether the window had no frame at all.

### 10 ms calls: CEF's pump time slice

The slowest `CefDoMessageLoopWork()` calls take 10.00 ms (10.003 ms max in r4). That is CEF's external pump time slice. CEF 8c29be2 `libcef/browser/browser_message_loop.cc:107` creates `MessagePumpExternal(0.01f, handler)`, and its `Run()` (lines 29 to 49) keeps running work until the next task is more than `max_time_slice_` (10 ms) past the call's start. When a delayed task is due within 10 ms, the call waits for it. So one pump call can hold GPUI's main thread for up to 10 ms, which is most of a 16.7 ms frame at 60 Hz. Phase 1's frame budget has to allow for it.

### Why CEF never asks for delayed work

`requests_delayed` is 0 in every run, by design. Chromium's `ThreadControllerWithMessagePumpImpl::SetNextDelayedDoWork` (`base/task/sequence_manager/thread_controller_with_message_pump_impl.cc:199` to `224`) only schedules delayed work when the thread is not inside `DoWork`, and nearly all delayed tasks are posted from inside it. CEF timers therefore rely on the 33 ms periodic call. The `delay > 0` path and the reentrancy guard are covered by unit tests (see "Unit tests").

## Page and GPUI animation together

Page `pages/anim.html`: a CSS `@keyframes` box and a `requestAnimationFrame` counter. Page counters were read over CDP at the window's edges. Each phase: 3 s warmup, 30 s window.

| Run | GPUI animation | Page rAF/s | CSS time advanced | OnPaint/s | GPUI renders/s | GPUI animation steps | CPU, tree |
|---|---|---|---|---|---|---|---|
| default | off | 30.05 | 30066 ms | 30.01 | 20.89 | 0 | 18.7 % |
| default | on | 30.01 | 30033 ms | 30.01 | 20.89 | 627 | 19.2 % |
| inactive throttle off | off | 30.01 | 30033 ms | 30.01 | 29.98 | 0 | 25.3 % |
| inactive throttle off | on | 30.02 | 30033 ms | 29.98 | 62.43 | 1874 | 46.6 % |

The page holds 30 frames/s in every case. `include/internal/cef_types.h` gives `windowless_frame_rate` "the default value is 30", and the spike does not change it. With default window options, GPUI draws about 21 frames/s. The window is never focused under Xvfb with no window manager, so GPUI's `inactive_frame_interval` (default 33.3 ms, "approximately 30 fps") applies. With `inactive_frame_interval: None` (`--pump-no-inactive-throttle`), GPUI shows every page frame, and draws more while its own animation runs. Its CPU numbers include those extra frames. On a focused window on a real session, the default throttle should not apply. That is unverified here.

## Agent access through `SendDevToolsMessage`

`results/2026-11-23-r4/cdp.json`:

- List tabs: 1 tab, `Target.getTargetInfo` returned `type: page`, title `anim`, the file URL.
- Evaluate: `Runtime.evaluate("navigator.webdriver")` back to back. The loop itself ran 30.000 s for a 30 s window: 540,555 calls, every reply `false`, no errors. Per-call latency: p50 0.038 ms, p99 0.12 ms, max 26.4 ms. The page counted `navigator.webdriver` on every animation frame: 900 samples during the loop, 0 not `false`, and its rAF rate over the loop was 30.00 frames/s.
- Screenshot: `Page.captureScreenshot` returned a 1024 × 640 PNG (`cdp.png`) in 109 ms.
- Correction to the first round: its report quoted 30.13 s, which was measured after the screenshot and the `/proc` scans, not the loop alone. The PR verifier reported 47.3, 33.4 and 34.7 s from that same field, and 903, 817 and 818 page frames. Only the frame counts show the page slowing down. r3 and r4 did not reproduce it: their loops ran 30.006 s and 30.000 s, at 30 page frames/s. The loop's length, per-call latency and page frame rate are now recorded separately, before the screenshot.
- Negative control (`results/2026-11-23/cdp-enable-automation/`, first-round build): the same check with `EXTRA_ARGS=--enable-automation`, 3 s warmup and a 5 s window. All 58,080 replies and all 153 page samples were `true`, and the run failed. The check detects `navigator.webdriver` turning on.
- No port or pipe: no process in the tree has `remote-debugging` or `enable-automation` on its command line. No process in the tree has a listening TCP socket. The run now fails unless every fd table and socket table was read (`listening_sockets.complete`), and in r4 all were. The one listening socket is Chromium's ProcessSingleton Unix socket (`/tmp/org.chromium.Chromium.*/SingletonSocket`), which is not a DevTools endpoint.

## DevTools and CEF #4353

CEF #4353: with every browser windowless and the app on `CefDoMessageLoopWork()`, closing a DevTools window started browser shutdown and killed renderers. The issue's own results: a first close 0 to 200 ms after `ShowDevTools` froze every page, and a close at 1 to 3 s froze the uninspected page. CEF fixed it in `11d50773` ("acquire an application keep-alive on the first CefDoMessageLoopWork call"). The pinned build `8c29be2` is 6 commits ahead of `11d50773` and 0 behind (GitHub compare).

Test: 2 windowless browsers on the animation page. Page 0 is inspected, page 1 is not. `ShowDevTools` gets a default (windowed) `CefWindowInfo` and a real client, `CloseDevTools()` runs N ms after the `ShowDevTools` call, and then each page is checked: `OnPaint` rate over 30 s, `Runtime.evaluate("1 + 1")`, renderer processes, `OnRenderProcessTerminated`. 5 fresh processes per delay.

| Close delay | Close after `ShowDevTools` | `ShowDevTools` call | Pages' OnPaint/s after | Evaluate after | Renderer terminations |
|---|---|---|---|---|---|
| 0 ms | 36 to 52 ms | 36 to 52 ms | 30, both pages | 2 and 2, all trials | 0 |
| 100 ms | 100 to 108 ms | 36 to 44 ms | 30, both pages | 2 and 2, all trials | 0 |
| 2000 ms | 2000 to 2005 ms | 36 to 60 ms | 30, both pages | 2 and 2, all trials | 0 |

`ShowDevTools()` is synchronous: it returns after creating the DevTools browser, and that took up to 60 ms. A close requested sooner than that runs when the call returns, so the 0 ms and 100 ms delays land later than asked. The DevTools browser was `CEF_RUNTIME_STYLE_CHROME` in its own X11 window. The `/proc` scan while DevTools is open now runs only if a scan as long as the one timed just before fits before the close. In r4 the scan was skipped in the 5 trials with a 0 ms delay and ran in the other 10. Each of those scans ended before the close was due: by 64 ms after `ShowDevTools` for the 100 ms delay, and by 87 ms for the 2000 ms delay. Those 10 closes still landed 0.04 to 8.0 ms late. The wait resumes on GPUI's main thread, where a single pump call can run for up to 10 ms, which likely accounts for that lateness.

Result: #4353 does not reproduce on CEF 158.0.3, and no workaround is needed. The check was not run against a build without the fix (for example CEF 156), so its ability to catch the freeze is not shown here.

## Shutdown

Each run closes every browser with `CloseBrowser(true)` and waits for every `OnBeforeClose`, which removes that browser's handle. Then it drops the observer registrations, quits GPUI, and calls `CefShutdown()`. If any browser is still open when the wait times out, the app skips `CefShutdown()` (calling it with live browsers is not allowed), records why, and fails the run. `CefShutdown()` returned in 48 to 122 ms. Right after it returns, some zygote processes are still alive. They exit when the browser process exits and closes their sockets. So after the app exits, the runner calls `cef-gpui-pump --pump-wait-gone=REPORT`. It waits on a pidfd for each child the app saw before shutdown, bounded by `STEP_TIMEOUT_MS`. A malformed child record, or a `/proc` or `pidfd_open` error other than "no such process", fails the check instead of counting as gone. In all 19 runs, every child had exited before that wait began.

Process types come from `--type=`, but zygote-forked processes keep `--type=zygote` on this build. `probe::Proc::kind` labels them by tree shape. The sandboxed layout seen here is browser, then the sandboxed zygote (PID-namespace init), then the zygote worker, then one process per renderer, each in its own PID namespace.

## Unit tests

`cargo test --release -p cef-gpui-pump` runs 9 tests without CEF: 7 in `src/schedule.rs` and 2 in `src/lib.rs`.

- A request with `delay = 10 ms` schedules a `Delayed` call at 10 ms. One with 50 ms is capped by a `Periodic` call at 33 ms.
- A new request replaces the pending one.
- A stream of delayed requests cannot push a call past 33 ms after the last one.
- A call made inside a running call is deferred, counted, and rerun after it.
- `analyze` accepts the scheduler's calls and flags a periodic call that came too early.

Each fix was checked by breaking it on purpose. Removing the reentrancy check fails the reentrancy test. Dropping the delayed path fails 2 tests. Letting a delayed request restart the 33 ms clock, as the first round's pump did, fails the stream test.

The 2 tests in `src/lib.rs` guard this report: `report_has_no_template_tokens` fails if REPORT.md still holds a fill token: an uppercase name wrapped in `@` signs, or one wrapped in the same digits on both sides, the shape a broken fill left in commit 7375b80 (`966` before and after each name). Run against that commit's REPORT.md, it fails and lists all of them. A guard in `run-checks.sh` would run before the report is written, and the fill is not a committed step, so the check lives in the test suite.

## Review findings

From the PR #93 verifier and 15 OpenCodeReview leads. "Reproduced" means shown on the first-round code (commit d7ade4a) before the fix. "By reading" means confirmed in the code but not run. Repro inputs and outputs are in `results/2026-11-23-r3/review-evidence/`.

| Finding | Reproduced? | Fix or reason |
|---|---|---|
| Runner had no scratch `XDG_RUNTIME_DIR`; dconf and the system bus were reached | Yes, negative control above | Scratch runtime dir, private bus on a random socket name in it, `DBUS_SYSTEM_BUS_ADDRESS` to a missing path, `isolation_clean` gates every run, `check-bus-listener.sh` gates the runner |
| Idle repaint cause | Confirmed by the verifier | Documented; idle passes with the paint-holding frame explained |
| 10 ms pump calls unexplained | Confirmed in CEF source | Documented, with the frame-budget impact |
| `requests_delayed` always 0 | Confirmed in Chromium source | Documented; unit tests cover the path |
| CDP loop length and page rate misreported | Reported by the verifier; the slowdown did not reproduce in r3 or r4 (30.0 s loops at 30 frames/s) | Loop length, per-call latency and page rAF rate recorded before the screenshot; report corrected |
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

Cargo 1.100's default build-dir layout keeps cef-rs's copy of CEF's runtime files out of the binary's directory (tauri-apps/cef-rs#522, open). Workaround: `scripts/run-checks.sh` sources Spike 1's `runtime-paths.sh`, checks the runtime that `prepare.sh` staged (its `archive.json` must match the `pins.toml` beta archive name and SHA-1, and `libcef.so` must be nonempty), and copies it with the binary into `target/release/cef-gpui-pump-bundle/<archive name>`. One bundle per archive keeps files from another CEF build out. The binary has `RPATH=$ORIGIN` (`build.rs`).

The binary's test target is off (`test = false`), because it links libcef and could only start from the bundle. Tests live in the CEF-free library. Cargo then reports the binary's dependencies as unused during `cargo test`, so `unused_dependencies` is allowed for this package.

The archive used is `cef_binary_158.0.3+g8c29be2+chromium-158.0.8127.18_linux64_beta_minimal.tar.bz2`, SHA-1 `057a0b871bf26d26d654d999dba595b92fe0f067` (`pins.toml` `[cef.beta.sha1]`, checked by `prepare.sh`; `results/2026-11-23-r4/cef-archive.json`). The r3 and first-round runs used the stable-channel archive of the same commit, SHA-1 `620b80a02dfe93ed7884a9eb1b265ff34f799024`, which unpatched cef-rs downloads by default.

## Dependencies

- One cef-rs source serves both spikes. `cef-dll-sys` declares `links = "cef_dll_wrapper"`, so Cargo allows only one copy of it in the workspace graph, and a separate git dependency on the same rev cannot sit next to Spike 1's path dependency. This spike takes `cef`, `gpui` and `gpui_platform` from `[workspace.dependencies]` in `rewrite/Cargo.toml`. Those point at `target/spike1-src/{cef-rs,zed}`, which `spikes/gpui-cef-compositor/prepare.sh` downloads at the `pins.toml` revs and patches. Each staged tree has a `.flecha-revision` stamp, and the patched `cef-dll-sys` build script fails unless both stamps equal the `pins.toml` repository and rev (cef-rs `Timpan4/cef-rs` `32d6bc55`, Zed `f126950f`). `Cargo.lock` has a single `cef-dll-sys` entry, a path package.
- The cef-rs patch changes CEF discovery in `sys/build.rs`, adds a `FLECHA_CEF_MINIMAL_SHA1` constant, and edits `osr_texture_import`, which is behind `accelerated_osr` and not built here. The Zed patch touches `gpui_linux`'s X11 and Wayland windows and `gpui_wgpu`, which this spike runs on.
- The workspace `cef` has no `accelerated_osr`, so cef-rs's optional registry `wgpu = "30"` is not in the graph. `Cargo.lock` has one wgpu, Zed's git rev `cddb2f5d`.
- The workspace `gpui_platform` enables `font-kit`, `x11` and `wayland`; the last two turn on `gpui_linux`'s display backends. `gpui` has `default-features = false`. On Linux its default features add nothing this spike needs (`font-kit` and `wayland` are empty, `x11` only reaches the optional `scap`).
- `[patch.crates-io]` in `rewrite/Cargo.toml` is main's: `calloop` at `zed-industries/calloop` `3759371f`, the same entry as Zed's `Cargo.toml` at `f126950f`. Zed at that rev also patches `async-task` (`smol-rs` `41fd3359`, a waker identity fix), `async-process` (`zed-industries` `0b6d6713`) and `khronos-egl` (`zed-industries` `42214b36`, no panic when a failed EGL call reports success). r3 applied those three, and this workspace does not: they resolve to crates.io `async-task 4.7.1`, `async-process 2.5.0` and `khronos-egl 6.0.0`. Adding them would replace entries in main's `Cargo.lock`, not only add this spike's.

## Differences from r3

r4 differs from r3 in more than the bus listener. Its build:

- Uses Spike 1's patched cef-rs and Zed instead of unpatched git dependencies on the same revs.
- Links and runs the beta archive instead of the stable archive of the same CEF commit.
- Takes `async-task`, `async-process` and `khronos-egl` from crates.io instead of Zed's forks.

Every pass condition held in both runs. Some figures moved. Idle pump calls went from 966 to 1,066: periodic calls stayed at 903 and 901, and immediate requests rose from 63 to 165. CDP evaluate calls in the 30 s loop went from 343,706 to 540,555, with p50 latency down from 0.062 to 0.038 ms. Without the inactive throttle and with GPUI animation on, GPUI renders went from 51 to 62 per second; the first round saw 30 or 62. There is one run per build, so this report does not attribute any shift to one of the three changes.

## Not verified

- Wayland, real X11 with a window manager, macOS and Windows. Everything ran on Xvfb.
- GPUI frame pacing on a focused window. Under Xvfb the window stays inactive.
- The pump's `delay > 0` path and reentrancy guard in the running app. Only the unit tests exercise them, because CEF sends no delayed requests here and nothing can re-enter the pump task.
- `CefShutdown()` being skipped after a browser-close timeout. No run timed out, and there is no hook to force one.
- Connections that open and close between the two isolation samples, and renderer sockets, which `ss` cannot resolve from the browser's network namespace.
- That the #4353 check fails on a build without the fix.
- Input, IME, resize and DPI changes. They are not in this spike's done check.
