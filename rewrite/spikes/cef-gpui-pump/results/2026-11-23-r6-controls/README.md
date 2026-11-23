# r6 negative controls

## Watchdog

`watchdog-control.sh watchdog`, run with `WARMUP_MS=3000 MEASURE_MS=5000 STEP_TIMEOUT_MS=5000`, runs the idle scenario through `scripts/run-checks.sh` with its full isolation and stops the app's browser process with `SIGSTOP` as soon as it exists. It selects that process as the `cef-gpui-pump` binary with this run's `--pump-out` and no `--type=` switch, because `timeout` and `dbus-run-session` carry the same arguments.

Result in `watchdog/`: the runner printed `watchdog expired after 38.000 s (exit 124)`, failed the run, wrote `idle.watchdog.txt` and exited 1. `control.log` shows no process left in the session's process group and no `cef-gpui-pump` or session process left anywhere. The run's scratch dir under `/tmp` was removed.

The first attempt matched `--pump-out` alone and stopped `timeout` itself, the oldest process with that argument. The session then finished and passed, so that attempt showed nothing about the watchdog, and its output was discarded.
