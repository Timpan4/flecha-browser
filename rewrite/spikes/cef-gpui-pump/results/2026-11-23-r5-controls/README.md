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

Controls 1 to 4 were applied to the checkout and reverted. From round 6 on,
every mutant runs in a disposable `git archive` copy of the committed tree,
deleted on exit, and never touches the caller's checkout:

- `idle-mutants.sh` applies each idle-rule mutant to `src/idle.rs` in a copy and
  runs the CEF-free `idle::` tests there through the build lock. It fails unless
  the unmutated rule passes and each mutant fails the tests named for it.
  `idle-mutants.out` is its output on d530b37: the unmutated rule passes 7 of 7,
  and all 5 mutants fail their named tests.
- `clippy-guard-control.sh` applies `direct-timer.patch`, control 1's per-call
  `executor.timer()` reapplied to the current bridge, and runs workspace
  `cargo clippy --locked --all-targets -- -D warnings` in a copy. It fails
  unless Clippy's diagnostic for `gpui::BackgroundExecutor::timer` has its
  primary span at the mutant's own call. `clippy-guard-control.out` is its
  output on 5cebba2: "use of a disallowed method
  `gpui::BackgroundExecutor::timer`" at `bridge.rs:230:64`, the mutant's call,
  exit 101. So control 1 passes `cdp_pass` at run time and Clippy rejects it
  at build time.

The ledger counts only timers armed through `Timers::arm`. A per-call timer
that calls `executor.timer()` directly, as in control 1, bypasses it and fails
`cdp_pass` only if its stall exceeds the `STEP_TIMEOUT_MS` bounds. Since round 6,
Clippy rejects such a call at build time (`clippy-guard-control.sh`).
