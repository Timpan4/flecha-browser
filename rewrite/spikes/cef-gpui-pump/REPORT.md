# Spike 2: CEF message pump in GPUI, and per-browser DevTools access

Date: 2026-11-23. Pins from `rewrite/pins.toml`: CEF `158.0.3+g8c29be2+chromium-158.0.8127.18` (beta-channel minimal archive), cef-rs `32d6bc55` (Timpan4/cef-rs), Zed/GPUI `f126950f`. cef-rs and Zed are the sources Spike 1's `prepare.sh` stages and patches (see "Dependencies").

Current results: `results/2026-11-23-r6/`, one full suite at `d530b37`, `results/2026-11-23-r6b/`, one cdp and one idle run at `5cebba2`, `results/2026-11-23-r6c/`, the same two at `70e99af`, `results/2026-11-23-r7/`, the same two at `8321ba3`, and `results/2026-11-23-r8/`, the same two at `ded93fb`, the last code change, which has every verdict rule this report describes (see "r6: the current head" and "r6b, r6c, r7 and r8"). The detailed figures in the sections below come from `results/2026-11-23-r5/`, the round before. r6 kept r5's pump and bridge behavior and changed verdicts, timer accounting, measurement fields and the harness, so r5's figures still describe the pump and the bridge; r6's are in its own section. `results/2026-11-23-r5-cdp-repeats/` holds 3 more r5 cdp runs with the browser's threads sampled. `results/2026-11-23-r5-controls/`, `results/2026-11-23-r6-controls/`, `results/2026-11-23-r7-controls/` and `results/2026-11-23-r8-controls/` hold the negative controls. `build.txt` in each names the commit and `cef-archive.json` the CEF archive. History, cited only where this report says so: `results/2026-11-23/` is the first round, `results/2026-11-23-r3/` the first fix round (its `review-evidence/` holds that round's repro inputs and outputs, not to be rerun), and `results/2026-11-23-r4/` the second. r4 used the CDP bridge that starved the pump (see "The CDP slowdown").

## Verdict

| Done check | Result |
|---|---|
| Pump runs only on CEF requests or CEF's documented periodic wake-up | Pass. 0 unexplained calls in every run. The pump trace now covers every scenario, and the largest gap between calls was 88.8 ms in r5 and 177.1 ms in r6, during a slow synchronous `ShowDevTools` call (see "r6: the current head") |
| Static page: GPUI draws no frames | Pass, with one explained frame. GPUI drew 1 frame for CEF's one paint-holding repaint at 4099 ms after navigation in r5 and 4098 ms in r6, and nothing else (see "Pump and idle") |
| Idle CPU (measurement only) | 0.53 % (r5) and 0.62 % (r6) of one core for the whole process tree |
| Page and GPUI animate together | Pass. Both frame counters advance |
| `navigator.webdriver === false` during CDP calls | Pass. 786,719 calls in 30.000 s in r5 and 798,831 in r6, 900 page samples each, 0 true. A control run with `--enable-automation` fails |
| CDP calls do not stall the page or the pump | Pass in 4 of 4 r5 cdp runs and the r6 cdp run after the fix. Before it, the bridge's per-call timers starved the pump (see "The CDP slowdown"). A control with the old timers fails |
| CDP without a port or pipe | Pass. No `remote-debugging` switch, no listening TCP socket in any of the tree's network namespaces, every fd and socket table read |
| DevTools close does not kill OSR renderers (CEF #4353) | Pass in 15 of 15 fresh processes in r5 and again in r6. No workaround needed |
| Clean shutdown | Pass. `CefShutdown()` returns in 48 to 117 ms (r5) and 43 to 66 ms (r6). Every child exited in all 19 runs of each |
| Runs isolated from the desktop session and the owner's home | Pass in 19 of 19 runs in r5 and in r6. Controls with the first round's environment, and with the scan pointed at the scratch home, fail |

## r6: the current head

`results/2026-11-23-r6/`, one full suite at `d530b37` with this report's knobs (`WARMUP_MS=3000`, `MEASURE_MS=30000`, `STEP_TIMEOUT_MS=20000`, `DEVTOOLS_CLOSE_MS="0 100 2000"`, `DEVTOOLS_TRIALS=5`), run once through the runner with its full isolation. 19 of 19 runs passed and the runner printed `all runs passed` (`run.log`). Each run's watchdog line in `run.log` gives its budget: 153 s idle, 266 s animate, 273 s cdp, 346 s, 346.1 s and 348 s devtools. No run came near it.

- Pump: 0 unexplained calls in every run, and the pump's ledger never counted more than 1 timer with an unexpired deadline. The largest gap between pump calls was 43.1 ms in idle, 44.5 ms in cdp, 51.1 and 54.7 ms in the animate runs, and 61.0 to 177.1 ms in the devtools runs. r5's devtools runs peaked at 88.8 ms. The larger gaps came in the runs whose synchronous `ShowDevTools` call, which blocks the GPUI thread, took longer: 36.8 to 133.4 ms in r6 against 36.6 to 40.6 ms in r5. r6 did not change that call or the pump, and why `ShowDevTools` took longer than r5's slowest call in 9 of the 15 r6 runs was not isolated. Every gap stayed far inside its bound of one `STEP_TIMEOUT_MS`.
- Idle: first paint at 135.4 ms after navigation, the paint-holding repaint at 4097.7 ms, inside the window of 4000 to 4168.4 ms (first paint + 4 s + `REPAINT_DELIVERY`). It came 37.7 ms before first paint + 4 s, earlier than the 27 ms of the earlier rounds; the rule's lower bound is navigation + 4 s, so earliness relative to first paint does not matter. GPUI drew 1 frame. The process tree used 0.62 % of one core (r5: 0.53 %).
- Animate: the page held 30.02 and 29.99 frames/s over its own intervals of 30,044 and 30,040 ms, with host windows of 30,024 and 30,020 ms. GPUI drew 20.85 and 20.89 frames/s with the default inactive throttle, and 30.01 and 62.46 frames/s without it. No `OnPaint` frame was overwritten before upload in any animate window. With the throttle, GPUI replaced 276 and 274 uploaded images before drawing them; without it, none.
- CDP: 798,831 `Runtime.evaluate` calls in 30.000 s, p50 0.029 ms, p99 0.11 ms, max 43.1 ms, 0 errors, 0 `true` replies. The page counted 900 rAF frames, 29.9998 frames/s over its own 30,000.2 ms against a 30,000.1 ms host window, and all 900 page samples were `false`. The bridge armed 3 timers with at most 1 deadline unexpired. The timeout probe returned after 20,000.03 ms, 0.03 ms past the call deadline and inside its 40 s bound (`within_deadline: true`). 1 `OnPaint` frame was overwritten before upload and 268 uploaded images were replaced before draw. The screenshot took 104.8 ms.
- Isolation: both samples in every run were clean, with 0 forbidden references and 0 unreadable entries. Each sample covered 9 processes (10 in the devtools runs, with the second browser) and 59 to 81 sockets. The cdp run read every fd and socket table (`listening_sockets.complete: true`, 0 fd entry errors).
- Shutdown: `CefShutdown()` returned in 43.3 to 65.6 ms, and every child process had exited before the wait began in all 19 runs (8 children in the idle, cdp and animate runs, 9 in the devtools runs).
- Controls for r6's changes: the watchdog control below, the Clippy guard control and the rewritten idle mutants in `results/2026-11-23-r5-controls/` (see "The CDP slowdown" and "Unit tests").

`results/2026-11-23-r6-controls/watchdog/` is the watchdog control, rerun at `70e99af`. `watchdog-control.sh` runs the idle scenario through the runner with `WARMUP_MS=3000 MEASURE_MS=5000 STEP_TIMEOUT_MS=5000`, a 38 s watchdog. It finds the run's browser through the runner's own process tree (the `cef-gpui-pump` process with no `--type=` switch below the runner it started), waits until the browser has a renderer, and records the session's tree: 10 processes, every one in `timeout`'s process group (`tree-before-stop.txt`). It then starts two helpers that ignore SIGTERM, as a Chromium child could: one joins that group, and one starts its own session with `setsid` but carries the run's `XDG_RUNTIME_DIR`. Then it stops the browser with `SIGSTOP`. `timeout` returned 124 once `dbus-run-session` had exited, while both helpers were still running: `idle.leftover.txt` lists both in state `S`. The runner's sweep ended them, the in-group helper by group ID and the other by the marker. The runner reported `watchdog expired after 38.000 s (exit 124); killed process group N` and exited 1, nothing in the group, the recorded tree or the helpers was left alive, and the run's scratch dir was gone. So `timeout` alone would have left both running (review threads 4666314344 and 4666506388), and its `--kill-after` `SIGKILL` never fired. What the control can see is the recorded tree, the group and the marker; a process that left the group and also dropped the run's `XDG_RUNTIME_DIR` would be outside all three.

`results/2026-11-23-r6-controls/startup-race/` is the startup control (`startup-race-control.sh`, review thread 4666506419). A `timeout` shim first on `PATH` records its PID and waits `STEP_TIMEOUT_MS` with a shell builtin before it execs the real `timeout`, which is what creates the session's process group. While the shim waits, still in the runner's own group, the control sends SIGTERM to the runner. At `70e99af` the runner exited 143, the shim was gone and no process of the run started. Against `af63dea`'s runner, from a disposable `git archive` copy (`startup-race-af63dea/`), the shim was still alive after the runner exited, so the control failed and killed the shim itself before it could exec `timeout`. Earlier versions of the control were discarded: one matched `--pump-out` alone and stopped `timeout` itself, and one stopped the browser about 100 ms after exec, before any Chromium child existed, so it showed the expiry but not a tree kill.

## r6b, r6c, r7 and r8

### r6b: `5cebba2`

`5cebba2` changed run-time code after the r6 suite. The isolation scan fails on any `ss` row it cannot parse and on scan roots not in normal form. The bridge returns a typed `CallTimeout`, which `cdp_pass` checks by type. `wait_gone` reports an out-of-range `step_timeout_ms` instead of panicking. The ledger's peak is renamed to say it counts unexpired deadlines (`*_timer_deadlines_max_unexpired`; the r5 and r6 reports use the old `*_timers_max_outstanding` names). The runner kills the session's process group itself. `results/2026-11-23-r6b/` holds one cdp and one idle run through the runner with this report's knobs, and both passed (`run.log`).

- CDP: 451,633 calls in 30.000 s, p50 0.031 ms, p99 0.35 ms, max 190.8 ms, 0 errors, 0 `true` replies. The page counted 29.60 rAF frames/s over its own 30,000.2 ms, and the largest pump gap was 190.5 ms. The bridge armed 3 timers, with at most 1 deadline unexpired, and the timeout probe returned after 20,000.05 ms (`within_deadline: true`). Every figure is inside its bound, but the loop made about 40 % fewer calls than the r5 and r6 runs, and its worst call and pump gap were about 4 times theirs. The host's 5-minute load average was 4.57 a minute later. A repeat right after (`results/2026-11-23-r6b-cdp-repeat/`) made 646,571 calls with a worst call of 45.1 ms, 30.00 page frames/s and a largest pump gap of 44.8 ms. Nothing this round changed runs inside the loop, and what slowed the first run was not isolated.
- Idle: first paint at 69.0 ms after navigation, the repaint at 4044.7 ms, 24.3 ms before first paint + 4 s and inside the window, which ended at 4102.0 ms. GPUI drew 1 frame, the tree used 0.76 % of one core, and the largest pump gap was 43.1 ms.
- Isolation: all 6 samples of the 3 runs were clean, with 0 unparsed `ss` rows and 0 unreadable entries. The forbidden roots canonicalize to themselves on this host.
- Shutdown: `CefShutdown()` returned in 47.5 to 53.1 ms, and all 8 children of each run had exited before the wait began.
- Controls: the watchdog control and the Clippy guard control both ran at `5cebba2`, and again at `70e99af`.

### r6c: `70e99af`

`70e99af` changed the scan and the runner again after a third review. The scan rejects an `ss` header that is not the full column list and scan roots that cannot be resolved for a reason other than not existing, or that resolve to a path that is not valid UTF-8. The runner refuses timing knobs past Bash's largest integer and budgets that would overflow, stops a session's launcher by PID before sweeping its group, also sweeps every process carrying the run's `XDG_RUNTIME_DIR`, and leaves zombies out of the sweep. `results/2026-11-23-r6c/` holds one cdp and one idle run through the runner with this report's knobs, and both passed (`run.log`).

- CDP: 755,585 calls in 30.000 s, p50 0.030 ms, p99 0.11 ms, max 18.5 ms, 0 errors, 0 `true` replies. The page counted 30.00 rAF frames/s over its own 30,000.2 ms, and the largest pump gap was 40.0 ms. The bridge had at most 1 deadline unexpired, and the timeout probe returned after 20,000.09 ms (`timed_out` and `within_deadline` true). The 5-minute load average was 2.07 right after.
- Idle: first paint at 65.2 ms after navigation, the repaint at 4042.8 ms, 22.4 ms before first paint + 4 s and inside the window, which ended at 4098.2 ms. GPUI drew 1 frame, the tree used 0.67 % of one core, and the largest pump gap was 43.1 ms.
- Isolation: all 4 samples clean, with 0 unparsed `ss` rows and 0 unreadable entries, so the full-header check accepts this host's `ss` output.
- Shutdown: `CefShutdown()` returned in 47.7 and 49.1 ms, all 8 children of each run had exited before the wait began, and the runner's sweep found nothing after either run.
- Controls at `70e99af`: the watchdog control, the startup control and the Clippy guard control.

### r7: `8321ba3`

`8321ba3` follows the merge of main (`186ae9d`, which brings in `c2fe8c1`, flecha-core) and changes only how the runner ends a run, after a fourth review. The commits after it change only comments, results and this report. Group members get no per-PID `kill`: the group kill covers them, sent while the inventory still lists a member of the group, since an empty group's ID can be reused once its leader is reaped. A marked member still alive at the sweeper's scan may also be signaled through its identity-checked pidfd. Processes carrying the run's `XDG_RUNTIME_DIR` are killed by the app binary's `--pump-sweep-kill` mode (`src/sweep.rs`), because Bash cannot open a pidfd: it opens a pidfd for each process the scan found, re-reads its start time and environment, and signals through that pidfd only if both still match, so a reused PID is skipped. A failed inventory, on the first pass or any retry, or a failed marker sweep is an incomplete sweep and fails the run. The EXIT trap sweeps by marker whenever a run's runtime dir is set, even if no group was recorded. The sweep code lives in `scripts/sweep.sh`. `results/2026-11-23-r7/` holds one cdp and one idle run through the runner with this report's knobs, and both passed (`run.log`).

- CDP: 616,169 calls in 30.000 s, p50 0.031 ms, p99 0.14 ms, max 35.1 ms, 0 `true` replies. The page counted 30.03 rAF frames/s, and the largest pump gap was 44.7 ms. The bridge had at most 1 deadline unexpired, and the timeout probe returned after 20,004.2 ms (`timed_out` and `within_deadline` true).
- Idle: first paint at 85.0 ms after navigation, the paint-holding repaint at 4047.3 ms, inside the window. GPUI drew 1 frame, the tree used 0.63 % of one core, and the largest pump gap was 43.1 ms.
- Shutdown: `CefShutdown()` returned in 50.8 and 47.2 ms, and the runner's sweep found nothing after either run.
- Controls in `results/2026-11-23-r7-controls/`:
  - `watchdog/`: the r6 watchdog control rerun against this runner. The in-group helper ended with the group kill, and the own-session helper, outside the group and carrying only the marker, ended through the pidfd sweep (`sweep: killed N` in `idle.leftover.txt`).
  - `ps-fails/` (`ps-fails-control.sh`): a `ps` that always fails, first on the runner's `PATH`. The app run passed, and the runner failed it because its inventory failed (exit 1).
  - `startup-race/`: the r6 startup control rerun against this runner's EXIT trap. The runner exited 143, the shim was gone and no process of the run started; both of the trap's sweeps ran (two sweeper lines in `run.log`). The control's generated `shim/timeout` was not kept.
  - `test-sweep.out` (`scripts/test-sweep.sh`, no CEF): `end_group` returns 3 when `ps` fails on the first pass, when it fails on a retry, and when the marker sweep fails. `stop_run`, called as the EXIT trap calls it, with a runtime dir but no group and no launcher in `jobs -p`, kills a marked process that is not a child of the shell.
- Every scan printed 4 of this user's processes whose `environ` it cannot read: `systemd --user`, `(sd-pam)`, `gpg-agent` and `scdaemon`. Being non-dumpable is the likely cause, which was not checked; the marker sweep cannot see them (see "Known limits").


### r8: `ded93fb`, the last code change

r8 fixes the independent verifier's two blockers at `56d12e9` and one of its notes, and merges main again (`9e0a7bc`, which brings in `dcee482`, CI workflow changes). The code changes are in `c4c0713` and `6cadc2a`; `ded93fb` adds controls and this report.

- Timer guard: `clippy.toml` lists every public GPUI API at `f126950f` that ends in a GPUI timer or a delayed dispatch, `gpui::FutureExt::with_timeout` and `gpui::PlatformDispatcher::dispatch_after` among them (see "The CDP slowdown" and "Phase 1 risk").
- Process discovery fails closed, and an inconclusive snapshot fails `isolation_clean`, the listener scan and `shutdown_clean` (see "Isolation"; review thread 4667057847).
- The session starts from an empty environment plus what the runner sets (see "Isolation").

`results/2026-11-23-r8/` holds one cdp and one idle run through the runner with this report's knobs, and both passed (`run.log`).

- CDP: 654,465 calls in 30.000 s, max 38.9 ms. The page counted 30.00 rAF frames/s, the largest pump gap was 48.5 ms, and the timeout probe returned after 20,000.04 ms. The listener scan was complete over 9 processes with no discovery error.
- Idle: first paint at 65.9 ms after navigation, the paint-holding repaint at 4041.5 ms, inside the window. GPUI drew 1 frame, the tree used 0.75 % of one core, and the largest pump gap was 58.0 ms.
- Isolation and shutdown: both isolation samples in each run covered 9 processes with no discovery error, and neither shutdown snapshot had one. `CefShutdown()` returned in 53.1 and 100.3 ms, and the runner's sweep found nothing after either run.

Controls in `results/2026-11-23-r8-controls/`, each on the committed tree:

- `timer-guard-control.sh`: workspace Clippy on a disposable copy passes unmodified, with no unresolved path, and rejects the verifier's mutant C at `bridge.rs:230:21` and each of the 13 listed paths at its own use in `timer_mutants.rs` (`timer-guard-control.out`).
- `discovery-control.sh`: mutant D (`root-stat-fails.patch`: the browser's own stat read fails with EMFILE), built in a disposable copy and run through the copy's runner. The verifier's dirty idle run (`EXTRA_ARGS=--pump-forbid-paths=/tmp`), which mutant D passed at `56d12e9`, now fails with `isolation_clean` and `shutdown_clean` false and discovery errors naming the missing root. A cdp run fails with `listening_sockets.complete` and `cdp_pass` false (`discovery-control.out`).
- `env-control.sh`: sets `SSH_AUTH_SOCK`, `AT_SPI_BUS_ADDRESS`, `PULSE_SERVER` and `XAUTHORITY` in the runner's environment and reads `/proc/PID/environ` during an idle run. With this runner (`env/`) none reached the browser or any of the 6 session processes; the browser held 9 variables, all set by the runner or `dbus-run-session`, and the run passed. With the runner at `6cadc2a`, from a disposable copy (`env-before-fix/`), the browser held 81 variables, all four of them included, and the control failed. Only variable names were recorded.

## What the spike is

`cef-gpui-pump` is one binary. It is the CEF browser process and every CEF subprocess, and it opens a GPUI window (`gpui_platform::application()`). The window shows one windowless, so Alloy-style, CEF browser. Its software `OnPaint` BGRA buffer becomes a GPUI `RenderImage`. Images GPUI drew are released with `Window::drop_image` once replaced. Frames replaced before GPUI drew them are dropped at once.

The library (`src/lib.rs`, no CEF, unit-tested) holds the rules the checks depend on:

- `src/schedule.rs` decides when to call `CefDoMessageLoopWork()`. It calls right away for `delay <= 0`, after the delay for `delay > 0`, and no later than 33 ms after the previous call ended. A new request replaces the pending one, as `OnScheduleMessagePumpWork` specifies, but cannot push the 33 ms deadline back. A request made before the last call started was served by that call. The 33 ms is from CEF 158 `include/cef_app.h`: "A maximum interval of approximately 33 milliseconds between calls (30 calls per second) is recommended." A guard defers a call made while one is running until it returns, as `cef_app.h` requires, and is released when the call unwinds. `analyze` checks a call log against those rules.
- `src/idle.rs` is the idle check's frame rule (see "The one frame: paint holding").
- `src/procfs.rs` parses the `/proc/PID/maps` and `ss -xan` text and reads fd tables for the isolation scan, keeping paths with spaces, recording every fd entry it could not resolve and every `ss` row it could not parse, and checking that the scan roots are absolute and in normal form.

The binary, Linux only (`src/main.rs` builds a stub that exits with "Linux only" elsewhere):

- `src/timers.rs` is the only place the crate arms a GPUI timer (`Timers::arm`), and each call goes into a ledger that counts timers created and estimates the most queued at once from their unexpired deadlines. The ledger cannot see the Timer thread's queue, so the Clippy guard below, not the count, is what keeps per-call timers out. The pump, the bridge and the scenario each own one. `clippy.toml` disallows every public GPUI API at `f126950f` whose call arms a GPUI timer or queues a delayed dispatch, so CI's Clippy run rejects a call to any of them outside `Timers::arm`. GPUI also arms timers itself, on some input and while an image source loads; Clippy cannot see those (see "Phase 1 risk" for the list and the audit).
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

Each run also has an outer watchdog, since `CefInitialize` and `CefShutdown` run outside every scenario deadline. `run-checks.sh` runs the session under `timeout`, which makes itself the leader of a new process group, runs the whole session in it and signals that group on expiry. `timeout` waits only for `dbus-run-session`, and sends `SIGKILL` one `STEP_TIMEOUT_MS` later only if that is still running, so the runner then kills whatever is left of the run itself and waits until nothing is, for at most `STEP_TIMEOUT_MS` (`scripts/sweep.sh`): the group by its ID, while the inventory still lists a member of it, and, through pidfds after re-checking each one's start time and marker, every process of this user whose readable environment holds the run's own scratch `XDG_RUNTIME_DIR`, which a process keeps if it leaves the group or is reparented. Every session process below `timeout` carries it: all 12 during one idle run, `dbus-run-session`, `dbus-daemon`, the browser and its sandboxed zygotes and renderers included, read through `/proc/PID/environ`. The sweep is not complete: a process that hides its environment or drops the variable is outside it (see "Known limits"). Zombies are already dead and left out. A failed inventory or marker sweep fails the run. It does the same after a run that ends normally, after the leak check so a slow child is still reported as a leak, and fails the run if anything of the session outlived it. Its EXIT trap, which INT and TERM also reach, ends a run in progress. `timeout` calls `setpgid(0, 0)` before it forks (coreutils `src/timeout.c`), so the trap first kills the launcher by PID, which stops a launch whose group does not exist yet, then sweeps; `jobs -p` finds the launcher even before the runner has recorded it, and the trap sweeps by marker whenever a run's runtime dir is set, even if neither the launcher nor its group was found. The timing knobs must be decimal digits no larger than Bash's largest integer, `MEASURE_MS` and `STEP_TIMEOUT_MS` must be nonzero as in the app, every budget is computed with overflow checks before anything starts, and timing flags in `EXTRA_ARGS` are refused, so the budget covers the durations the app runs. The budget comes from the run's own knobs: one warmup plus one measurement per measured window, plus one `STEP_TIMEOUT_MS` per wait the app bounds and per synchronous CEF call it cannot bound (`CefInitialize`, `CreateBrowserSync` per page, `ShowDevTools`, `CefShutdown`), counted from the code (`watchdog_ms` in the runner). With this report's knobs that is 153 s for idle, 266 s for animate, 273 s for cdp and 346 s plus the close delay for devtools. A run that hits it fails, `NAME.watchdog.txt` records it, `NAME.leftover.txt` lists what the group kill found, and cleanup runs as usual. `results/2026-11-23-r6-controls/watchdog/` shows it ending a hung run and two SIGTERM-ignoring processes, one in its group and one outside it, and `startup-race/` shows the trap stopping a launch (see "r6: the current head").

Build: release profile. GPUI picked `NVIDIA GeForce RTX 5080 (Vulkan)` on Xvfb, so GPUI was not on a software renderer (`results/2026-11-23-r4/adapter-smoke/`, a 5 s idle run with `RUST_LOG=gpui_wgpu=info` on the r4 build). CEF itself used software OSR (`OnPaint`, no shared textures). Chromium switches: `--ozone-platform=x11 --password-store=basic`. The sandbox was on.

### Isolation

Each app run gets one `mktemp -d` base (mode 700) under `/tmp`, deleted afterwards. It is created under `/tmp` rather than the caller's `TMPDIR`, and its path is checked to hold only letters, digits, `/`, `.` and `-`, because the bus config and the bus address embed it unescaped. It holds:

- the CEF cache;
- `XDG_RUNTIME_DIR`;
- `HOME`;
- `TMPDIR`.

The run also gets:

- An empty environment (`env -i`) plus only what the runner sets: `PATH=/usr/bin:/bin`, `LANG=C.UTF-8`, `DISPLAY` for Xvfb, `XDG_SESSION_TYPE=x11`, the scratch `XDG_RUNTIME_DIR`, `HOME` and `TMPDIR`, and `DBUS_SYSTEM_BUS_ADDRESS`; `dbus-run-session` adds the private bus address. Before r8 the runner unset a list of variables and passed the rest of the caller's environment: 81 variables reached the browser in the environment control, `SSH_AUTH_SOCK`, `AT_SPI_BUS_ADDRESS`, `PULSE_SERVER` and `XAUTHORITY` among them (`results/2026-11-23-r8-controls/env-before-fix/`).
- Xvfb (`-nolisten tcp`) on a display it picks.
- `dbus-run-session` with a config written from `scripts/isolated-session-bus.conf.in`: no service directories, so nothing is activated, and `<listen>unix:dir=SCRATCH</listen>`, so dbus-daemon creates a socket with a new random name in that run's scratch runtime dir. The first fix round listened at `$XDG_RUNTIME_DIR/bus` (`unix:runtime=yes`). A test with two buses on one scratch dir showed that listener replacing an existing socket at its path and deleting it on exit, so pointed at the desktop's runtime dir it would remove `/run/user/1000/bus`. `scripts/check-bus-listener.sh` starts a second bus on the same dir while the first runs and requires the first socket to be the same file and still answering afterwards. The runner runs it before any app run. It passes with the new listener and fails with the old one ("second bus reused …/bus", "first bus socket … is gone").
- `DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent`.

The app checks this itself. After the page loads and again before shutdown, `probe::forbidden_refs` lists every file the process tree has open or memory-mapped and every Unix socket peer (from `ss -xan`). It fails the run if any lies under the caller's `XDG_RUNTIME_DIR` (`/run/user/1000`), `/run/dbus/system_bus_socket`, or the caller's real `HOME`, except the bundle, the test pages and the results dir, which the runner passes as allowed (`--pump-allow-paths`). Paths match by whole components, and `--pump-forbid-paths` and `--pump-allow-paths` reject relative entries and spellings not in normal form (`/run//user/1000`, `/run/user/./1000`, a trailing `/`), which the component match could never meet. The kernel reports fd targets and mapped files with symlinks resolved, so a root that exists is also added in its canonical form, and a symlinked root covers what it points to (`procfs::scan_roots`). A root that cannot be resolved for any reason other than not existing, or whose canonical form is not valid UTF-8 and so could not be matched exactly, is an error naming the option. Map paths and `ss` addresses keep their spaces (`procfs::maps_path`, `procfs::ss_unix_row`). Every fd entry the scan could not resolve, whether it vanished mid-scan or could not be read, marks the scan incomplete and fails `isolation_clean`. So does any `ss` row the parser rejects, since its socket would go unchecked, and a header that is not the full column list, which may mean truncated output (`ss_unparsed_rows`). In r5 all 38 samples were clean, with every fd table and map readable and no `ss` error. The limits: it samples twice, so a connection opened and closed between samples is missed, and sandboxed renderers live in their own network namespace, so their sockets (12 to 22 per sample) get no peer path from `ss`. Their open and mapped files are still scanned.

Process discovery fails closed (`src/proctree.rs`, since r8). A `/proc` listing that cannot be read, a stat that cannot be read for a reason other than the process being gone (`NotFound` or `ESRCH`), a stat that does not parse, or a missing root makes the snapshot inconclusive. An inconclusive snapshot fails `isolation_clean`, the listener scan's `complete` (and so `cdp_pass`) and `shutdown_clean`, and each report lists the errors as `discovery_errors`. At `56d12e9` the verifier's mutant D, whose root stat read failed, scanned 0 processes and passed a run made dirty on purpose (review thread 4667057847).

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

The parent agent's decision: the idle check passes with that frame explained. The rule is `idle::judge` in the library. The paint-holding repaint is the first full-view paint that follows the navigation's first paint, comes at least `kNewContentRenderingDelay` after navigation, and comes no later than `kNewContentRenderingDelay` plus `REPAINT_DELIVERY` after that first paint. Only the first such paint counts, over every paint since navigation, not only the window. `REPAINT_DELIVERY` is the pump's 33 ms `MAX_PUMP_INTERVAL` (`cef_app.h`). Chromium starts the timer when the navigation commits, before the first paint, so the repaint is due by first paint + 4 s, and CEF can deliver it only inside a pump call. Measured: the repaint came 3973 (r3), 3976 (r4) and 4002 ms (r5) after the first paint, and 3995 to 4007 ms in the first round's 4 runs, so between 27 ms early and 7 ms late; r6's came 3962 ms after, 38 ms early. The upper bound is the one that matters for a late repaint, and no run came later than 7 ms. `idle_pass` requires 0 unexplained pump calls in the window, every paint in the window to be that repaint, and no more GPUI renders than CEF paints in the window. `idle_frames.strict_no_frames` records whether the window had no frame at all.

The second fix round counted the allowance inside the window only, so a lone full-view repaint at 8 s passed when the real repaint fell before the window. Its rule also lived in the binary, which has no tests. The third counted it over the whole navigation but had no upper bound, so a lone full-view repaint at 8 s still passed when no earlier repaint was logged (review thread 4665850156). See "Unit tests" for the cases and mutants.

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

Since r6 the page rates divide by the page's own clock between its two counter reads (`performance.now()`, `page_counter_interval_ms`), next to the host window (`seconds`), so page frames outside the counted interval cannot raise the rate. Each window also reports `on_paint_frames_overwritten_before_upload`, page frames the next `OnPaint` replaced before GPUI took them, and `gpui_images_replaced_before_draw`, uploaded images replaced before GPUI drew them. They are measurements; no verdict uses them.

The page holds 30 frames/s in every case. `include/internal/cef_types.h` gives `windowless_frame_rate` "the default value is 30", and the spike does not change it. With default window options, GPUI draws about 21 frames/s. The window is never focused under Xvfb with no window manager, so GPUI's `inactive_frame_interval` (default 33.3 ms, "approximately 30 fps") applies. With `inactive_frame_interval: None` (`--pump-no-inactive-throttle`), GPUI shows every page frame, and draws more while its own animation runs. Its CPU numbers include those extra frames. On a focused window on a real session, the default throttle should not apply. That is unverified here.

## Agent access through `SendDevToolsMessage`

`results/2026-11-23-r5/cdp.json`:

- List tabs: 1 tab, `Target.getTargetInfo` returned `type: page`, title `anim`, the file URL.
- Evaluate: `Runtime.evaluate("navigator.webdriver")` back to back. The loop ran 30.000 s for a 30 s window: 786,719 calls, every reply `false`, no errors. Per-call latency: p50 0.029 ms, p99 0.11 ms, max 43.0 ms. The page counted `navigator.webdriver` on every animation frame: 900 samples during the loop, 0 not `false`, and its rAF rate over the loop was 30.00 frames/s.
- Timeout under load: while the loop runs, a second call evaluates `new Promise(() => {})` with `awaitPromise`, which never replies. The bridge failed it after 20000.0 ms, 0.03 ms past its 20 s deadline. `cdp_pass` requires it to time out, recognized by the bridge's `CallTimeout` error type since r6b rather than by its message, and since r6 also requires the measured time to stay within the probe's bound of two `STEP_TIMEOUT_MS` (`cdp_timeout_probe.within_deadline`), so a timeout reply picked after both futures were ready cannot count.
- Screenshot: `Page.captureScreenshot` returned a 1024 × 640 PNG (`cdp.png`) in 105 ms.
- Negative control (`results/2026-11-23/cdp-enable-automation/`, first-round build): the same check with `EXTRA_ARGS=--enable-automation`, 3 s warmup and a 5 s window. All 58,080 replies and all 153 page samples were `true`, and the run failed. The check detects `navigator.webdriver` turning on.
- No port or pipe: no process in the tree has `remote-debugging` or `enable-automation` on its command line. No process has a listening TCP socket. The check reads `/proc/PID/net/{tcp,tcp6,unix}` once per network namespace in the tree (the `/proc/PID/ns/net` link tells them apart), so sandboxed renderers' namespaces are covered. In each cdp run it read 2 namespaces. The run fails unless every fd table and socket table was read (`listening_sockets.complete`), and in r5 all were. The one listening socket is Chromium's ProcessSingleton Unix socket (`$TMPDIR/org.chromium.Chromium.*/SingletonSocket`, under the run's scratch `TMPDIR`), which is not a DevTools endpoint.

`cdp_pass` also requires the loop to end within the window plus `STEP_TIMEOUT_MS` (50 s here), every call to return within `STEP_TIMEOUT_MS`, and the bridge never to hold more than one GPUI timer whose deadline has not passed (`bridge_timer_deadlines_max_unexpired` since r6b, `bridge_timers_max_outstanding` in the r5 and r6 reports). The ledger counts nominal deadlines and cannot see the Timer thread's queue, so this is an estimate; the Clippy guard is what keeps per-call timers out. This run's bridge armed 3 timers, with at most 1 deadline unexpired at once.

## The CDP slowdown

The first round reported 30.13 s for the 30 s loop, measured after the screenshot and the `/proc` scans. The PR verifier then saw 47.3, 33.4 and 34.7 s from that field, with 903, 817 and 818 page frames. r3 and r4 recorded the loop alone and ran 30.006 s and 30.000 s, and the second fix round reported that the slowdown did not reproduce. It does. The verifier ran the r4 code (a7a92bd) 5 times: the loop took 49 to 57 s, the worst call 22 to 30 s, and the page dropped to 14 to 16 frames/s, while `cdp_pass` stayed true.

### Cause

GPUI timers on Linux all run on one shared thread and are not cancelled when dropped:

1. `BackgroundExecutor::timer(d)` with a nonzero `d` goes through `dispatcher.dispatch_after` (`zed/crates/gpui/src/platform_scheduler.rs:129-150`).
2. On Linux that inserts a calloop `Timer` source into the event loop of the single `Timer` thread (`gpui_linux/src/linux/dispatcher.rs:54-90`).
3. Dropping the returned `Task` does not remove the source. It stays until its deadline and then fires into nothing.

The r4 bridge raced every call against a fresh 20 s `executor.timer(timeout)`. At about 15,000 calls a second, that kept about 300,000 timers on the Timer thread. The verifier sampled the browser's threads once a second: about 5 s into the loop the Timer thread reached 100 % of a core, and about 27 s in the main thread sat idle in `epoll_wait`. The pump's own 33 ms timer was stuck behind that backlog, so CEF went unpumped. The bridge's timeout could not fire either, because its timer sat in the same queue. With the per-call timer replaced by a future that never fires, the loop ran 30.000 s with 561,307 calls, a 35 ms worst call and 30.0 page frames/s.

### Fix

- `src/bridge.rs`: a call records a deadline instead of arming a timer. One sweeper task sleeps until the earliest deadline in flight, or until a call arrives when none is, and fails the calls past their deadline. Deadlines are the call time plus one fixed timeout, so a call made while the timer is set never has an earlier deadline, and the sweeper arms about one timer per timeout. Every bridge timer goes through one ledger, `Timers::arm`, which counts those created and the most whose deadlines had not passed at once.
- `src/pump.rs`: the pump re-armed a timer for every request. It now keeps the armed timer while a request is not due sooner, which brings it to about 30 timers a second.

### Measurements

The Timer thread's CPU, sampled once a second as the verifier did (`threads.log`; 100 ticks are one core):

| Run | Build | Loop | Worst call | Page frames/s | Bridge timers created / max unexpired deadlines | Timer thread | `cdp_pass` |
|---|---|---|---|---|---|---|---|
| r5 | fixed | 30.000 s | 43.0 ms | 30.00 | 3 / 1 | not sampled | true |
| Repeat 1 | fixed | 30.000 s | 27.8 ms | 30.03 | 3 / 1 | no ticks in 33 samples | true |
| Repeat 2 | fixed | 30.000 s | 19.0 ms | 30.03 | 3 / 1 | no ticks in 33 samples | true |
| Repeat 3 | fixed | 30.000 s | 43.1 ms | 30.00 | 3 / 1 | active in 1 of 33 samples, 1 tick | true |
| Control 1 | per-call timer, before the ledger | 38.325 s | 11316.6 ms | 21.21 | not counted | active in 38 of 42 samples, max 102, mean 87 ticks/s | true |
| Control 2 | per-call timer and a pump timer per request, before the ledger | 30.004 s | 33.4 ms | 30.0 | not counted | active in 29 of 33 samples, max 103, mean 97 ticks/s | true |
| Control 3 | per-call timer through the ledger | 37.937 s | 10936.5 ms | 21.42 | 503,445 / 374,654 | active in 37 of 50 samples, max 102, mean 84 ticks/s | false |

Controls are in `results/2026-11-23-r5-controls/`, each run once through the runner with its full isolation. Each applies its patch from that dir to the committed code.

The bounds the parent asked for, a call over `STEP_TIMEOUT_MS` or a loop over the window plus `STEP_TIMEOUT_MS`, did not catch the controls here. The controls ran on the same host as the verifier, but with this round's pump (lazy timers, request draining, a yield after each call) and the old bridge timer, not a7a92bd's code. Their stalls were shorter than the verifier's (worst calls of 11 s against 22 to 30 s) and intermittent: control 2 saturated the Timer thread and still ran on time. Which difference shortened them was not isolated. So `cdp_pass` also checks the cause directly: the bridge may never have more than one timer deadline unexpired at once, which is the sweeper's design. Control 3 fails on that and passes the latency bounds. Why r4's single run did not stall is not known.

The ledger sees only timers armed through `Timers::arm`. A per-call timer armed with `executor.timer()` directly, as control 1 does, bypassed it and passed `cdp_pass` at run time. Since r6, `Timers::arm` is the only place the crate may call GPUI's timer: `clippy.toml` disallows `gpui::BackgroundExecutor::timer` and both `scheduler_executor` methods, and only `Timers::arm` carries `#[allow(clippy::disallowed_methods)]`. Control 1's mutant, reapplied to the current bridge (`results/2026-11-23-r5-controls/direct-timer.patch`), fails workspace `cargo clippy --locked --all-targets -- -D warnings` with "use of a disallowed method `gpui::BackgroundExecutor::timer`" at the mutant's own call, `bridge.rs:230:64`, the span the control requires (`clippy-guard-control.sh`, `clippy-guard-control.out`, rerun at `5cebba2`). So control 1 passes `cdp_pass` at run time and is rejected at build time. The two `scheduler_executor` entries were checked once in a disposable copy, not kept as a control: calls to both in `Bridge::new` failed with "use of a disallowed method" for each, and a third entry naming a method GPUI lacks drew the warning "does not refer to a reachable function", so Clippy 1.100 checks that each configured path resolves.

That list was incomplete. The verifier's mutant C, a per-call `gpui::FutureExt::with_timeout` in `Bridge::call`, passed Clippy and `cdp_pass` at `56d12e9`, with an 11 s stall in one of three runs; `with_timeout` calls `executor.timer()` inside GPUI. Since r8, `clippy.toml` lists every public entry point the audit in "Phase 1 risk" found. `results/2026-11-23-r8-controls/timer-guard-control.sh` runs workspace Clippy on a disposable copy of the committed tree: the unmodified copy passes with no unresolved path, and with mutant C (`with-timeout.patch`) and one use of each listed path (`timer_mutants.rs`) added, Clippy rejects mutant C at `bridge.rs:230:21` and each of the 13 paths at its own use (`timer-guard-control.out`).

### Phase 1 risk

GPUI timers on Linux share one thread and are not cancelled on drop. Any per-request timer pattern in Flecha, such as timeouts in the MCP server, CDP calls or downloads, must not create one GPUI timer per request. Keep one timer per component, set for the earliest deadline, as the bridge does now, or use a timer source that removes cancelled timers. A Timer thread backlog delays every GPUI timer in the process, including the CEF pump's. This spike enforces the rule with Clippy `disallowed-methods` and `disallowed-fields` entries for every public GPUI API at `f126950f` that ends in a GPUI timer or a delayed dispatch, and one allowed call site. Phase 1 can reuse that list for the code it writes, rechecked on each GPUI update. It does not cover the timers GPUI arms itself, listed below.

The audit read every `.timer(`, `dispatch_after`, `Timer::after` and `with_timeout` call in the Zed crates in the spike's dependency graph (gpui, gpui_linux, gpui_platform, gpui_wgpu, gpui_util, gpui_shared_string, gpui_macros, scheduler, collections, sum_tree, http_client, refineable, derive_refineable, ztracing), and traced each to the public API that reaches it:

- Guarded: `BackgroundExecutor::timer`. `BackgroundExecutor::scheduler_executor`, `ForegroundExecutor::scheduler_executor` and `SchedulerLocalExecutor::timer`, the scheduler executors' timers. `FutureExt::with_timeout` (`util.rs:64-80`). `PlatformDispatcher::dispatch_after` (`platform.rs:1594`), reachable through the doc(hidden) `dispatcher()` accessors, which the Linux dispatcher queues on the Timer thread (`dispatcher.rs:135`). `Animation::with_max_fps` and the `max_fps` field, a timer per frame (`animation.rs:445`). `Interactivity::tooltip`, `Interactivity::hoverable_tooltip`, `StatefulInteractiveElement::tooltip` and `StatefulInteractiveElement::hoverable_tooltip`, timers per hover (`div.rs:3996`, `div.rs:4119`). `App::prevent_idle_sleep`, a 10 s timer per call (gpui_linux `platform.rs:643-660`).
- Not guardable by Clippy: `img()` arms a timer while its source loads asynchronously (`img.rs:409`); the spike passes `ImageSource::Render`, which is ready at once. A window arms one timer per touch for long-press (`window.rs:6448`) and one per multi-keystroke binding sequence (`window.rs:6743`).
- Test code only: the test and visual-test contexts, the tests in `util.rs` and `threaded_dispatcher.rs`, `gpui_platform.rs:179` and gpui_linux `power.rs:175`. `InteractiveText::tooltip` arms no timer. The other crates have no timer call.

Two more Phase 1 risks from review, not changed here:

- A cancelled bridge call keeps its pending entry until its deadline (review thread 4665850142), because only the receiver is dropped. Here every call is awaited to completion. An agent server that cancels requests needs a drop guard that removes the entry.
- The pump drains every queued CEF request before it picks the next call, with no bound (summary item `pump.rs:165`). A producer that keeps the queue full could keep the pump from checking its deadline or yielding to GPUI. CEF's requests did not do that in any run here, but Phase 1 should bound the drain by its own deadline.

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

Each run closes every browser with `CloseBrowser(true)` and waits for every `OnBeforeClose`, which removes that browser's handle. Then it drops the observer registrations, quits GPUI, and calls `CefShutdown()`. If any browser is still open when the wait times out, the app skips `CefShutdown()` (calling it with live browsers is not allowed), records why, and fails the run. `CefShutdown()` returned in 48 to 117 ms. Right after it returns, some zygote processes are still alive. They exit when the browser process exits and closes their sockets. So after the app exits, the runner calls `cef-gpui-pump --pump-wait-gone=REPORT` under `env -i` with the run's scratch `HOME` and `TMPDIR`. It waits on a pidfd for each process in either snapshot the app took, the children before shutdown and the descendants right after `CefShutdown()`, deduplicated by PID and start time, bounded by `STEP_TIMEOUT_MS`. A malformed child record, or a `/proc` or `pidfd_open` error other than "no such process", fails the check instead of counting as gone. In all 19 runs, every child had exited before that wait began.

Process types come from `--type=`, but zygote-forked processes keep `--type=zygote` on this build. `probe::Proc::kind` labels them by tree shape. The sandboxed layout seen here is browser, then the sandboxed zygote (PID-namespace init), then the zygote worker, then one process per renderer, each in its own PID namespace.

## Unit tests

`cargo test --release -p cef-gpui-pump` runs 39 tests without CEF: 11 in `src/schedule.rs`, 7 in `src/idle.rs`, 7 in `src/procfs.rs`, 7 in `src/proctree.rs`, 4 in `src/sweep.rs` and 3 in `src/lib.rs`.

`src/proctree.rs` tests discovery on fake `/proc` dirs: a complete listing gives the tree below the root, root first. A missing root, an entry whose stat cannot be read (a directory in its place, which fails with EISDIR whatever the test's privileges), a stat that does not parse, the root's own stat failing to parse, and a `/proc` dir that cannot be listed each make the tree inconclusive. A process whose stat is gone by the time it is read is left out without an error. On the live `/proc`, the test process is a complete root.

`src/sweep.rs`: a stat line whose `comm` holds spaces and parentheses parses, and the marker matches whole environment entries only. A re-read whose start time differs from the scan's is skipped (`StartTimeChanged`), as are a zombie, a missing process and one that lost the marker. A live test starts a marked `sleep`, finds it by scan, and calls the kill with its start time plus 1, as a reused PID would show: the kill skips it and the child keeps running. With the scanned start time the kill sends SIGKILL through the pidfd, and the child dies by signal 9.

`src/schedule.rs`:

- A request with `delay = 10 ms` schedules a `Delayed` call at 10 ms. One with 50 ms is capped by a `Periodic` call at 33 ms.
- A new request replaces the pending one. A stream of delayed requests cannot push a call past 33 ms after the last one.
- A call made inside a running call is deferred, counted, and rerun after it. The guard is released when the call unwinds.
- `analyze` accepts the scheduler's calls and flags a periodic call that came too early. One request explains only one call, a request made before the last call started counts as served, and a window keeps the trace before it as history.

Each fix was checked by breaking it on purpose. Removing the reentrancy check fails the reentrancy test. Dropping the delayed path fails 2 tests. Letting a delayed request restart the 33 ms clock, as the first round's pump did, fails the stream test.

`src/idle.rs` tests the frame rule with synthetic paint logs. The r4 and r5 logs, a first paint and then one repaint at 4099 ms, pass. These fail: a second repaint, a full-view repaint at 3.5 s, a repaint after an earlier paint-holding repaint before the window, a lone repaint at 8 s, a repaint 1 ms past the delivery bound, a repaint that is the first paint, a partial repaint, an extra GPUI render and an unexplained pump call. Mutants, each applied to a disposable `git archive` copy of the committed tree, never the caller's checkout (`results/2026-11-23-r5-controls/idle-mutants.sh`, output in `idle-mutants.out`). The script fails unless the unmutated rule passes its idle tests and each mutant fails the tests named for it:

| Mutant | Tests that fail |
|---|---|
| Accept two paint-holding repaints (`candidates <= 2`) | 2, including the second repaint |
| `PAINT_HOLDING_TIMEOUT` = 0 s | 4, including the r4 and r5 logs |
| Allowance counted inside the window only (the second round's rule) | 1: the repaint after an earlier one |
| No upper bound on the repaint time (the third round's rule) | 1: the lone repaint at 8 s |
| No earlier paint required | 1: the repaint that is the first paint |
| None | 0 of 7 |

`src/procfs.rs` tests the isolation scan's parsing: a map path and an `ss` address under `/home/Test User` keep their spaces and match that forbidden root, and an fd directory with one symlink and one entry `readlink` cannot resolve gives one target, one failure and an incomplete table (review thread `probe.rs:284`). An `ss` table with a row missing its peer inode, one with no header and empty output are all rejected rather than skipped (4666314421). Relative and non-normal roots are rejected naming the option, and a symlinked root also yields its target (4666314387). A truncated header such as `Netid State` is rejected, and the full header with any spacing is accepted (4666506440). A root under a mode 000 dir and a symlink to a dir whose name is not valid UTF-8 are errors naming the option (4666506455, 4666506429); the permission case is skipped when the test process can read a mode 000 dir. The tests that create symlinks are `#[cfg(unix)]`, so the parser tests still build for Windows (4666314405).

`src/lib.rs`: `path_under` matches whole path components, which the isolation scan's allow-list relies on. `report_has_no_template_tokens` fails if REPORT.md still holds a fill token: an uppercase name wrapped in `@` signs, or one wrapped in the same digits on both sides, the shape a broken fill left in commit 7375b80 (`966` before and after each name). Run against that commit's REPORT.md, it fails and lists all of them. A guard in `run-checks.sh` would run before the report is written, and the fill is not a committed step, so the check lives in the test suite.

## Review findings

From the PR #93 verifiers and the OpenCodeReview threads. "Reproduced" means shown on the code before the fix. "By reading" means confirmed in the code but not run. Repro inputs and outputs for the first fix round are in `results/2026-11-23-r3/review-evidence/`; its `run.sh` scripts are marked historical and exit at once.

| Finding | Reproduced? | Fix or reason |
|---|---|---|
| A per-call timer armed with `executor.timer()` bypasses the ledger (parent, control 1) | Yes, control 1 passed `cdp_pass` | `Timers::arm` is the only allowed timer call; Clippy rejects control 1 at build time |
| fd readlink failures ignored by the isolation scan (`probe.rs:284`, 4665850168) | By reading; unit test with a synthetic failure | Failures mark the scan incomplete and fail `isolation_clean` |
| Map and `ss` paths truncated at a space (4665850181) | By reading: the old `split_whitespace().nth(5)` turns the test's path into `/home/Test` | `procfs::maps_path` and `procfs::ss_unix_row` keep the rest of the line |
| Relative forbid or allow paths accepted (4665850151) | By reading | Rejected, naming the option |
| `wait_gone` ignores processes first seen during `CefShutdown` (4665850161) | By reading | Both snapshots checked, deduplicated by PID and start time |
| Timeout probe verdict ignores the measured time (4665850204) | By reading | `cdp_timeout_probe.pass` requires the return within two `STEP_TIMEOUT_MS` |
| A lone late repaint counts as paint holding (4665850156) | Yes: the 8 s test fails on the "no upper bound" mutant, the third round's rule | Upper bound of first paint + 4 s + `MAX_PUMP_INTERVAL` |
| Page rAF rate divided by the host interval (4665850190) | By reading | Divided by the page's own clock; both intervals reported |
| No outer deadline around `CefInitialize` and `CefShutdown` (4665850134) | By reading | Per-run watchdog from the run's knobs; `SIGSTOP` control |
| Scratch path from the caller's `TMPDIR` reaches `sed`, XML and the bus address (4665850125) | By reading | Scratch dirs under `/tmp`, path checked |
| Linux-only code breaks non-Linux workspace checks (4665850088) | By reading (`libc::SYS_pidfd_open`); no non-Linux target installed | Linux modules and dependencies gated; stub elsewhere |
| Mutant script edited the caller's checkout and could pass on a failure (4665850097, 4665850104, 4665850114) | By reading | Disposable copy, argument and tool checks, named-test checks, cargo status checked |
| Overwritten paints not counted (`view.rs:29`, 4665850225) | By reading | `on_paint_frames_overwritten_before_upload` and `gpui_images_replaced_before_draw` |
| `ss` rows the parser rejects are never checked (4666314421) | By reading | Rejected rows fail the scan (`ss_unparsed_rows`); unit test |
| Non-normal scan roots never match (4666314387) | By reading: `path_under` compares literal prefixes | Rejected, naming the option; existing roots also added canonicalized; unit tests |
| `cdp_pass` recognizes the timeout by its message (4666314433) | By reading | Typed `CallTimeout`, checked by type |
| `timeout` waits only for `dbus-run-session` (4666314344) | Yes: the control's SIGTERM-ignoring group member was still running when `timeout` returned 124 | The runner kills the group by ID after every run and checks that it is empty |
| Watchdog control finds the browser by output path (4666314308) | By reading | Found through the runner's own process tree; checks scoped to the run's group and recorded tree |
| fd-table test breaks Windows test builds (4666314405) | By reading; no Windows target here | Symlink tests are `#[cfg(unix)]` |
| Octal knobs and timing flags in `EXTRA_ARGS` (4666314332) | By reading | Knobs parsed as decimal; timing flags in `EXTRA_ARGS` refused |
| Out-of-range `step_timeout_ms` panics (4666314356) | By reading | `Duration::try_from_secs_f64` with context |
| Clippy guard control matches lint and file separately (4666314290) | By reading | Matches the diagnostic's header and primary span at the mutant's line and column |
| The ledger's peak is not a queue measurement (4666314438) | By reading | Renamed to `*_timer_deadlines_max_unexpired` and described as an estimate |
| `idle-mutants.sh` path dependencies not pinned to `HEAD` (4666314301) | By reading | Known limit |
| Watchdog control discovery deadline (4666314322) | By reading | Known limit |
| Watchdog statuses can come from the session (4666314351) | By reading | Known limit; old markers removed at run start |
| Idle 33 ms delivery cutoff (4666314392) | By reading | Known limit |
| A transient fd `NotFound` fails the scan (4666314412) | By reading | Known limit |
| Cleanup can miss a session still being launched (4666506419) | Yes: the startup control fails on `af63dea`'s runner | The EXIT trap, reached from INT and TERM, kills the launcher by PID before the sweep; `jobs -p` finds it before it is recorded |
| The watchdog control exits under `set -e` and leaves its helper (4666506399) | By reading | An EXIT trap kills and reaps every child the control still owns |
| Survivor checks miss processes that leave the group (4666506388) | By reading; no Chromium process left it in the samples | The runner and the control also find the run's processes by their `XDG_RUNTIME_DIR`; a helper in its own session is swept |
| A truncated `ss` header is accepted as an empty table (4666506440) | By reading | The header must be the full column list; unit test |
| Permission and I/O errors resolving a root are treated as missing (4666506455) | By reading | Only `NotFound` keeps the literal root; other errors name the option; unit test |
| Lossy canonical roots (4666506429) | By reading | A canonical root that is not valid UTF-8 is an error; unit test |
| Bash overflow in the knobs and the budget (4666506408) | Yes: `STEP_TIMEOUT_MS=18446744073709551616` read as 0 in Bash | Knobs past Bash's largest integer refused, nonzero step and window required, budgets overflow-checked before Xvfb starts |
| Clippy control's span matched as a substring (4666506379) | By reading | The stripped span line must equal the expected one |
| The procfs test dirs are named by PID only (4666506467) | By reading | Known limit |
| Stale PIDs get SIGKILL after the group kill (4666737162) | By reading | No per-PID `kill` for group members; the group kill covers them. Marked processes are killed through a pidfd after the start time and marker are re-checked (`--pump-sweep-kill`). Unit test with a stale start time |
| A failed `ps` inventory reads as a clean sweep (4666737142) | Shown with `test-sweep.sh` and `ps-fails-control.sh` on the fixed runner | Each inventory's status is checked, on the first pass and every retry; a failure is an incomplete sweep and fails the run |
| A TERM before the group is recorded skips the marker sweep (4666737126) | By reading | The EXIT trap sweeps by marker whenever a runtime dir is set; `test-sweep.sh` case 4 |
| Checked-in shims write to the original checkout (4666737062, 4666737110) | By reading | Not executable; their first line marks them as historical captures the control regenerates |
| Non-dumpable processes hide the marker (4666737135) | Yes: 4 of this user's processes hide `environ` on this host | Known limit; per-run cgroup for Phase 1 |
| Wall-clock cleanup deadline (4666737152) | By reading | Known limit |
| `kill -0` counts zombies in the watchdog control (4666737120) | By reading | Known limit |
| `canonicalize` error test skipped under privileges (4666737174) | By reading | Known limit |
| Startup control robustness (4666737069, 4666737078, 4666737085, 4666737091, 4666737099) | By reading | Known limit |
| The timer guard misses `gpui::FutureExt::with_timeout` (verifier, mutant C) | Yes, by the verifier: mutant C passed Clippy and `cdp_pass`, with an 11 s stall in one of three runs | Every public GPUI timer API at `f126950f` disallowed; timer guard control |
| Process discovery fails open (4667057847) | Yes, by the verifier: mutant D passed a run made dirty on purpose | Discovery fails closed; isolation, listener and shutdown verdicts fail on discovery errors; unit tests; discovery control |
| The browser inherits the caller's environment (verifier) | Yes: with the old runner, all four canaries reached the browser | `env -i` with a runner-set allowlist; environment control |
| Group-ID reuse after the leader is reaped (4667057781) | By reading; the verifier's numbers | Known limit |
| A newline in argv forges a sweep record (4667057920) | Yes, by the verifier | Known limit |

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
- `wait_gone` treats any nonzero pidfd event as exit, so a latent `POLLNVAL` or `POLLERR` would count as gone.
- Two invocations into one results dir can delete or replace each other's reports.
- `--pump-wait-gone` panics on a malformed report instead of failing cleanly.
- A malformed `/proc/PID/stat` reads as "process exited" in `wait_gone`.
- `pump.rs:80`: a request dequeued just before the idle window could flag its call just after as unexplained. `analyze_between` now keeps the earlier trace, which closes this case too.
- `forbidden_refs` waits for `ss` with no deadline, on the GPUI thread. The per-run watchdog now bounds the run as a whole.
- The schedule analyzer keeps a request the scheduler would ignore as already served (review thread 4665850212). A request stamped before the previous call but enqueued after a fresh immediate request can replace it in the analysis, and the next call is then flagged unexplained. That gives a false failure, never a false pass. No run hit it.
- The cdp scenario writes its screenshot on the GPUI thread while the pump trace runs (summary item `scenario.rs:439`). Slow storage could stretch a pump gap and fail `pump_pass`; it cannot make one pass. The whole screenshot step, capture and write, took 88 to 105 ms in the 4 r5 cdp runs.
- Each tree snapshot reads every visible process before keeping the descendants (summary item `probe.rs:136`). That adds unrelated `/proc` reads to timed runs on a busy host. Since r8 an unreadable or unparseable stat of any visible process, not only the tree's, makes the snapshot inconclusive: a false failure, never a false pass.

Limits of this round's changes:

- The `ss` parser takes the peer address as one token, which iproute2 7.3's `ss` prints as `*` for every Unix socket (1,531 of 1,531 rows on this host).
- An fd that closes between the scan listing its directory and reading its link fails `isolation_clean`, with no retry (review thread 4666314412). That is a false failure by design; no run hit it.
- The non-Linux stub was not compiled here: only the `x86_64-unknown-linux-gnu` target is installed, and adding one changes the user's toolchain.
- Fixed this round, and so dropped from this list: the missing per-run deadline (the watchdog), the `sed` case for a `TMPDIR` with `&` or `|` (scratch under `/tmp`, path checked), and the `gpui_frames_never_drawn` name (now `gpui_images_replaced_before_draw`, next to `on_paint_frames_overwritten_before_upload`).

From the second review of round 6, kept without a change because each gives a false failure or affects robustness, never a false pass:

- `idle-mutants.sh` archives `HEAD` but uses the caller's staged `target/spike1-src` path dependencies, which `--locked` does not verify, so a tree staged for another revision could be what it tests (4666314301).
- The watchdog control's discovery wait starts before the runner's setup (runtime check, bundle copy, Xvfb), so slow setup can make it fail without testing the watchdog (4666314322). Its abort path stops the runner, whose EXIT trap ends the run's group.
- The runner reads exit 124 or 137 as the watchdog, but the session could exit with either itself, which would mislabel a crash and skip its leak check; the run still fails (4666314351). Old markers are removed at the start of each run.
- The idle rule's upper bound adds only the pump's 33 ms `MAX_PUMP_INTERVAL`, while idle pump gaps reached 43 ms, so a repaint behind a late pump call near the bound fails the run (4666314392). No run came within 26 ms of the bound.
- The procfs tests that create directories name them by PID only, so a directory left by an interrupted run whose PID comes round again makes the next run's test fail before it checks anything (4666506467). A false failure in the tests.
- The runner's sweep finds a run's processes by group and by the run's `XDG_RUNTIME_DIR`. A process that left the group and also dropped that variable from its environment would be outside both; none was seen.

From the fourth review of round 6, kept without a change:

- The marker sweep sees only processes whose `environ` it can read, and Linux hides it for a non-dumpable process, so the sweep is not complete (4666737135). The answer for Phase 1 is per-run cgroup containment, such as a transient systemd scope, which holds a process however it leaves the group.
- An incomplete sweep returns before it signals anything, so a failed inventory can leave the run's processes alive. The run fails and names the incomplete sweep.
- The cleanup deadline uses wall-clock time (`EPOCHREALTIME`) and starts after the first scan, so a backward clock step or a slow scan lengthens cleanup (4666737152).
- The watchdog control's `alive` uses `kill -0`, which counts a zombie as alive, so an unreaped orphan gives a false failure (4666737120).
- The test that `canonicalize` errors other than `NotFound` reach the caller is skipped when the tests run with privileges that can read a mode 000 dir (4666737174).
- The startup control can mislead with a shared `OUT_DIR`, an `OUT_DIR` holding shell or regex characters, a runner that hangs or ignores SIGTERM, or a shim that execs between the group sample and SIGTERM (4666737069, 4666737078, 4666737085, 4666737091, 4666737099). Its checked-in shims are historical captures and no longer executable (4666737062, 4666737110).

From the verifier at `56d12e9` (round 8), kept without a change:

- `run_one` reaps the `timeout` leader before `end_group`, so nothing reserves the group ID, and a group kill could reach a new group that took the ID after the run's group emptied (4667057781). The verifier read pid_max 4,194,304 and measured 108 PIDs/s during the suite, about 10.8 h per wrap, against runs of at most 348 s. Phase 1's launcher should contain each run in a cgroup or signal only through pidfds.
- A newline in a marked process's argv can forge a `pid pgid` line in `--pump-sweep` output, which can drive a group kill for a group with no member; its harm depends on the group-ID reuse above (4667057920).
- Operator inputs can weaken the checks: `EXTRA_ARGS=--pump-forbid-paths=...` replaces the runner's forbidden roots, because the app keeps a repeated flag's last value, and `DEVTOOLS_TRIALS=0` runs no devtools trial yet prints `all runs passed`.
- The idle rule accepts an empty paint log.

Review notes the parent kept without a change:

- `schedule.rs:73`: the 33 ms fallback counts from the previous call's end, not its start, so a 10 ms call puts the next start about 43 ms later. The idle trace shows that maximum.
- `scenario.rs:490`: the 0 ms DevTools close is issued after `ShowDevTools` returns, so no trial closes DevTools while its creation is pending.

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
