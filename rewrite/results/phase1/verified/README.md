# Phase 1 Linux proof

Both Xvfb runs passed on 2026-11-23. The [build stamp](build-source.txt) identifies the tested source revision, Rust compiler, and binary. CEF and GPUI use the versions in [pins.toml](../../../pins.toml).

- [Run 1](run-1.log) launched with caller-provided `SSH_AUTH_SOCK`, `AT_SPI_BUS_ADDRESS`, `PULSE_SERVER`, and `XAUTHORITY` pointing at synthetic paths. The live app and its CEF descendants had none of those variables.
- [Run 2](run-2.log) repeated the slice in a new scratch profile. Both runs quit and restarted within their own profile.
- [Split](split.png), [Peek](peek.png), and [Flechinha](flechinha.png) are Xvfb captures from run 1. The split shows the actual X11 keystroke sent to the first page. The Flechinha capture shows the draft preserved across the move.
- [Negative controls](controls.log) caught missing restore data, cookie sharing, a stale browser callback, direct startup, and a GPUI `with_timeout` call. Each browser control ended its isolated process namespace. The direct-start control exited before CEF initialization.

The locked workspace tests passed 123 tests. Workspace Clippy with all targets and warnings denied, `cargo fmt --check`, ShellCheck, `git diff --check origin/main HEAD`, and `npm run check:source` passed. The installed Rust target was `x86_64-unknown-linux-gnu`.
