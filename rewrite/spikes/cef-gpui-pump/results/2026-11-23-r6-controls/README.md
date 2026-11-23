# r6 negative controls

## Watchdog

`watchdog-control.sh watchdog`, run at 5cebba2 with `WARMUP_MS=3000 MEASURE_MS=5000 STEP_TIMEOUT_MS=5000`, runs the idle scenario through `scripts/run-checks.sh` with its full isolation. It finds the run's browser through the runner's own process tree: the `cef-gpui-pump` process with no `--type=` switch below the runner it started. Once the browser has a renderer, it records the session's tree with each process's group, adds a process that ignores SIGTERM to the session's process group, and stops the browser with `SIGSTOP`.

Result in `watchdog/`:

- `tree-before-stop.txt`: 10 session processes, every one in `timeout`'s process group.
- `idle.leftover.txt`: what the runner's group kill found after `timeout` returned 124. The SIGTERM-ignoring process was still running (state `S`), beside the session's zombies, so `timeout` alone would have left it alive.
- `run.log`: `watchdog expired after 38.000 s (exit 124); killed process group N`, and the runner exited 1.
- `control.log`: no process left in the group or in the recorded tree, and the SIGTERM-ignoring process was gone after the runner. The run's scratch dir under `/tmp` was removed.

Earlier versions were discarded. One matched `--pump-out` alone and stopped `timeout` itself, the oldest process with that argument, so the session finished and passed. One stopped the browser about 100 ms after exec, before any Chromium child existed, so it showed the expiry but not a tree kill.
