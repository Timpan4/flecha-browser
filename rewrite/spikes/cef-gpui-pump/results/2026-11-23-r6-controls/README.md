# r6 negative controls

Both run at 70e99af with `WARMUP_MS=3000 MEASURE_MS=5000 STEP_TIMEOUT_MS=5000`, through `scripts/run-checks.sh` with its full isolation.

## Watchdog

`watchdog-control.sh watchdog` runs the idle scenario. It finds the run's browser through the runner's own process tree: the `cef-gpui-pump` process with no `--type=` switch below the runner it started. Once the browser has a renderer, it records the session's tree with each process's group and starts two helpers that ignore SIGTERM: one joins the session's process group, and one starts its own session with `setsid` but carries the run's `XDG_RUNTIME_DIR`. Then it stops the browser with `SIGSTOP`. An EXIT trap kills and reaps every child the control still owns on any exit.

Result in `watchdog/`:

- `tree-before-stop.txt`: 10 session processes, every one in `timeout`'s process group.
- `idle.leftover.txt`: what the runner's sweep found after `timeout` returned 124. Both helpers were still running (state `S`), so `timeout` alone would have left them alive.
- `run.log`: `watchdog expired after 38.000 s (exit 124); killed process group N`, and the runner exited 1.
- `control.log`: no process left in the group or in the recorded tree, and both helpers gone after the runner. The run's scratch dir under `/tmp` was removed.

Scope: the control sees the recorded tree, the group and the marker. A process that left the group and dropped the run's `XDG_RUNTIME_DIR` would be outside all three.

Earlier versions were discarded. One matched `--pump-out` alone and stopped `timeout` itself, so the session finished and passed. One stopped the browser about 100 ms after exec, before any Chromium child existed, so it showed the expiry but not a tree kill.

## Startup

`startup-race-control.sh OUT_DIR [RUNNER]` puts a `timeout` shim first on `PATH`. The shim records its PID and waits `STEP_TIMEOUT_MS` with a shell builtin before it execs the real `timeout`, which is what creates the session's process group. While it waits, the control sends SIGTERM to the runner, then checks that the shim is gone and that no process of the run started. If the shim survived, the control kills it by PID before it can exec `timeout`.

- `startup-race/`: the current runner exited 143, the shim was gone and nothing started.
- `startup-race-af63dea/`: `af63dea`'s runner, run from a disposable `git archive` copy, left the shim alive, so the control failed and killed the shim itself.
