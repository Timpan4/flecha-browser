# Phase 1 guard verification

The [build stamp](build-source.txt) identifies the tested commit and binary. [Run 1](run-1.log) seeded synthetic caller variables. [Run 2](run-2.log) used an independent scratch profile. Both runs reopened their own profile after quitting.

- The split check reads the native X11 window dimensions, the chrome edges, and the split ratio. It compares every screenshot pixel with two solid fixture rectangles, allowing for the column crossed by a fractional split boundary. The checks ran [before restart](split-geometry.png) and [after restart](restart-split-geometry.png).
- The returned tab navigated to a page with a distinct URL and title before close. The archive retained its previous entries and gained the new page with a later timestamp.
- A direct Rust test injected every CEF event variant under a retired browser key and checked that the dispatcher dropped each event. The test's exhaustive match ties sample coverage to the event type.

All [17 failure controls](controls.log) tripped their intended assertions, including the verifier's width, height, duplicate archive, and stale-event mutants.

The first pane-height attempt exited during launcher startup, before Flecha ran. The retry used the same mutant binary. The remaining controls used the same shadow checkout.

The 210 locked workspace tests, workspace Clippy with all targets and warnings denied, `cargo fmt --check`, ShellCheck, `npm run check:source`, and `git diff --check origin/main HEAD` passed.
