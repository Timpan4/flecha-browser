# r5 negative controls

Each control ran once through `scripts/run-checks.sh`, with its full isolation
(Xvfb, private bus, scratch XDG_RUNTIME_DIR, HOME and TMPDIR). Timing knobs:
`WARMUP_MS=3000 MEASURE_MS=30000 STEP_TIMEOUT_MS=20000` (control 4: `MEASURE_MS=5000`).
`build.txt` in each directory names the commit and the tracked edits the
binary was built with. `threads.log` holds one sample a second of the browser
process's threads (state, wchan, CPU ticks since the previous sample).

| Control | Build | Result |
|---|---|---|
| 1-per-call-timer-latency-bounds-only | da80fd4 plus `per-call-timer.patch` (bridge timer per call) | `cdp_pass` true. The loop stalled to 38.3 s, worst call 11.3 s, both under the `STEP_TIMEOUT_MS` bounds. Timer thread active in 38 of 42 samples, max 102 ticks/s (100 ticks are one core) |
| 2-per-call-and-per-request-timers-latency-bounds-only | da80fd4 plus `per-call-and-per-request-timers.patch` (also a new pump timer on every pass) | `cdp_pass` true. No stall this time: 30.004 s, worst call 33 ms. Timer thread active in 29 of 33 samples, max 103 ticks/s (100 ticks are one core) |
| 3-per-call-timer | c6c2f00 plus `per-call-timer.patch`, the timer armed through the bridge's ledger | `cdp_pass` false: `bridge_timers_max_outstanding` 374,654 (the fixed build: 1). The loop stalled to 37.9 s, worst call 10.9 s |
| 4-home-scan-detects | c6c2f00 with `scripts/run-checks.sh` plus `home-scan-control.patch`, which also forbids the run's scratch HOME | `isolation_clean` false: the scan found `home/.cache/mesa_shader_cache/index` and `home/.cache/radv_builtin_shaders/index` mapped, so the same files under the real home would fail a run |

Controls 1 and 2 ran before the timer ledger existed. They show that the
latency bounds alone miss an intermittent stall. Control 2's pump change keeps
the current loop's request draining and yield, so it is not a7a92bd's pump.

`idle-mutants.sh` applies each idle-rule mutant to `src/idle.rs`, runs the
CEF-free `idle::` tests through the build lock, and restores the file with git.
`idle-mutants.out` is its output on c6c2f00's `src/idle.rs` (unchanged since):
every mutant fails at least one test, the unmutated rule passes 5 of 5.
