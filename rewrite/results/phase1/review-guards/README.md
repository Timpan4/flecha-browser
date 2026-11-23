# Phase 1 guard verification

The [build stamp](build-source.txt) identifies the tested commit and binary. [Run 1](run-1.log) seeded synthetic caller variables. [Run 2](run-2.log) used an independent scratch profile. Both runs reopened their own profile after quitting.

- The split check reads the native X11 window dimensions, the chrome edges, and the split ratio. It compares every screenshot pixel with two solid fixture rectangles, allowing for the column crossed by a fractional split boundary. The checks ran [before restart](split-geometry.png) and [after restart](restart-split-geometry.png).
- The returned tab navigated to a page with a distinct URL and title before close. The archive retained its previous entries and gained the new page with a later timestamp.
- A direct Rust test injected every CEF event variant under a retired browser key and checked that the dispatcher dropped each event. The test's exhaustive match ties sample coverage to the event type.

## Partition and capture recheck

The [recheck build stamp](recheck-build-source.txt) identifies the code and binary used for the [ten consecutive Xvfb runs](recheck-runs.log). Each run checked that clicking the covered sidebar row dismissed Peek, changing a Space partition closed both transients, and the launcher removed its scratch tree after CEF exited. The first restart capture needed another read of the X root before both pages appeared.

All [27 failure controls](controls.log) tripped their expected checks. The UI stale-timestamp mutant failed the close-time assertion. The blank-restart mutant rendered a unique replacement color and reached the watchdog. Its failed run retained the screenshot and app log. The controls removed their shadow build on success.

The controls ran from `rewrite/` with `FLECHA_HARNESS_TIMEOUT_MS=30000 tools/verify-negative-controls.sh`. The override uses the existing watchdog and allows three times the verifier's longest clean baseline run.

Locked workspace tests (210), workspace Clippy with all targets and warnings denied, `cargo fmt --all --check`, `npm run check:source` (312 tests), and the repository's ShellCheck command passed.
