# r8 controls

The verifier's blockers at `56d12e9`, shown fixed. Each control works on the committed tree or through `scripts/run-checks.sh` with its full isolation. The scripts are not executable; run them with `bash`. The app runs used `WARMUP_MS=3000 MEASURE_MS=5000 STEP_TIMEOUT_MS=5000`.

## Timer guard

`timer-guard-control.sh REWRITE_DIR` (at `6cadc2a`, output in `timer-guard-control.out`) runs workspace Clippy with `-D warnings` on a disposable `git archive` copy:

1. Unmodified: passes, and no `clippy.toml` path is unresolved.
2. With `with-timeout.patch` (the verifier's mutant C, a per-call `gpui::FutureExt::with_timeout` in `Bridge::call`) and `timer_mutants.rs` (one use of each listed path): Clippy rejects mutant C at `bridge.rs:230:21` and each of the 13 listed paths at its own use.

## Process discovery

`discovery-control.sh REWRITE_DIR OUT_DIR` (at `c4c0713`, output in `discovery-control.out`, runs in `discovery/`) builds mutant D (`root-stat-fails.patch`: the browser's own `/proc/PID/stat` read fails with EMFILE) in a disposable copy and runs the copy's runner:

- The verifier's dirty idle run (`EXTRA_ARGS=--pump-forbid-paths=/tmp`) fails: `isolation_clean` and `shutdown_clean` false, discovery errors naming EMFILE and the missing root.
- A cdp run fails: `listening_sockets.complete` and `cdp_pass` false.

## Environment

`env-control.sh OUT_DIR [RUNNER]` sets `SSH_AUTH_SOCK`, `AT_SPI_BUS_ADDRESS`, `PULSE_SERVER` and `XAUTHORITY` in the runner's environment, runs idle, and reads `/proc/PID/environ` of the browser and of every readable process below the run's `timeout`. It records variable names only.

- `env/` (this runner, at `c4c0713`): the control passed. The browser held 9 variables (`browser-environ-names.txt`), none of the four, and neither did any of the 6 session processes. The run passed.
- `env-before-fix/` (the runner at `6cadc2a`, from a disposable copy deleted afterwards): the control failed. The browser held 81 variables, all four included.
