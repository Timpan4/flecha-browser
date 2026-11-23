# Phase 1 review verification

The [build stamp](build-source.txt) identifies the source and binary tested on 2026-11-23. [Run 1](run-1.log) used synthetic caller session variables. [Run 2](run-2.log) used a fresh scratch profile. Both runs painted the fixture pages on the expected sides [before restart](split.png) and [after restart](restart-split.png).

- The TypeScript replay matched the independent verifier's trace byte for byte. Each Xvfb run compared live state with that trace after open, split, promote, detach, return, and close. Close added a new archive entry.
- The stale-event check queued Created, Paint, Title, and Address for a retired browser generation. The replacement loaded and painted, and the shell received none of the retired events.
- The shared Space held a distinct cookie before Peek opened in an isolated Space with no tabs. Peek saw no cookie. The isolated Space's own cookie persisted across restart.
- The launcher found no filesystem sockets before starting its services. NSS resolved localhost from the private hosts file and returned no entry for `localtest.me`. CEF rejected navigation to that public name.

The [negative controls](controls.log) include the verifier's swapped panes and blank restart mutants, stale event delivery, a skipped close archive, and a Peek that uses shared cookies. They also check that the isolation probe rejects a socket or public-name alias planted inside the sandbox.

The workspace's 162 Rust tests, workspace Clippy with all targets and warnings denied, `cargo fmt --check`, ShellCheck on changed scripts, `npm run check:source`, and `git diff --check origin/main HEAD` passed. The Cargo lock diff against main adds 53 lines and removes none.
