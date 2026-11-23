# r7 controls

All ran at `8321ba3`. The two app runs used `WARMUP_MS=3000 MEASURE_MS=5000 STEP_TIMEOUT_MS=5000`, through `scripts/run-checks.sh` with its full isolation. The scripts are not executable; run them with `bash`.

## Watchdog

`watchdog/`: `results/2026-11-23-r6-controls/watchdog-control.sh` rerun against this runner (`bash .../watchdog-control.sh results/2026-11-23-r7-controls/watchdog`). The control passed. `idle.leftover.txt` lists both SIGTERM-ignoring helpers. The in-group helper ended with the group kill, which is the only signal group members get now. The own-session helper, outside the group and carrying only the run's `XDG_RUNTIME_DIR`, ended through the pidfd sweep: `sweep: killed N`.

## Failed inventory

`ps-fails/`: `ps-fails-control.sh` puts a `ps` that always fails, with no output, first on the runner's `PATH`, and runs the idle scenario. The app run passed (`exit=0 ... pass=true`). The runner reported `the sweep of process group N and its marked processes was incomplete` and exited 1, and `idle.leftover.txt` says `sweep incomplete: the inventory failed`.

## Sweep functions

`test-sweep.out`: `STEP_TIMEOUT_MS=5000 scripts/test-sweep.sh <bundled cef-gpui-pump>`, which sources `scripts/sweep.sh` and starts no CEF. Its marked processes carry a runtime dir it makes, and each starts its own session with `setsid -f`, so none is a job of the shell under test. Cases:

1. `ps` fails on the first pass: `end_group` returns 3.
2. `ps` succeeds once and then fails on the retry: `end_group` returns 3.
3. The marker sweep fails: `end_group` returns 3.
4. `stop_run`, called as the EXIT trap calls it with a runtime dir but no group and nothing in `jobs -p`, lists the marked process and kills it through the sweeper.

A first version of the test hung because each marked process kept the stdout of the caller's command substitution open. Stopping it ran its EXIT trap, which killed the marked process through the sweeper and removed its work dir. Nothing was left.
