# Flecha rewrite: GPUI frontend on CEF

The plan, the decisions behind it, and what each phase must prove are in [docs/cef-gpui-rewrite-plan.md](../docs/cef-gpui-rewrite-plan.md). `chromium/` is still the shipping browser. Nothing here changes it.

## Layout

- `pins.toml`: CEF, cef-rs and GPUI versions. All crates use these pins.
- `crates/<name>`: product crates (`flecha-core`, `flecha-cef`, `flecha-surface`, `flecha-ui`, `flecha-services`).
- `spikes/<name>`: Phase 0 go/no-go spikes. Each spike has a `REPORT.md` with its done check and results.

A unit adds its crate path to `members` in `Cargo.toml`.

The compositor spike uses patched copies of the pinned Zed and cef-rs sources.
From `rewrite/`, run `bash spikes/gpui-cef-compositor/prepare.sh` before resolving
the workspace with Cargo. Stage the CEF runtime explicitly because Cargo can
reuse build-script results without rerunning the script. Preparation uses
`flock` to serialize staging and verifies the pinned CEF beta archive. Never run
`cargo`, `rustc`, `rustup` or Clippy inside `rewrite/target/spike1-src/zed`.
The [spike report](spikes/gpui-cef-compositor/REPORT.md) describes the runtime
and the compositor checks.

## Building on the workstation

The workstation runs one heavy build at a time. Use `~/.cargo/bin/cargo`
because `cargo` on `PATH` can be a wrapper shim. Every `cargo build`, `cargo test`
or `cargo run` that compiles CEF or GPUI waits for the shared lock and runs in the
agent build slice:

```sh
flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
  bash ~/.claude/skills/hosts/scripts/agent-build run ~/.cargo/bin/cargo build --locked -j 14 -p flecha
```

Run it from `rewrite/`, with Cargo's target dir left at `rewrite/target`. If a build is killed for memory, check `free -g` before retrying.

## Isolated CEF launches

Run every CEF launch through an isolated launcher, including tamper and negative
tests. Use Xvfb or headless Weston, a private session D-Bus with service
activation disabled, and scratch directories for `XDG_RUNTIME_DIR`, `HOME` and
`TMPDIR`. Set `DBUS_SYSTEM_BUS_ADDRESS` to a nonexistent socket path. Never use
the owner's desktop session, session bus or `/run/user/<uid>/bus`. Stop the
compositors and browsers you start by PID.

Do not run `sudo`, `pkexec`, or host system-bus or polkit probes such as
`resolvectl statistics` or `systemctl` on system units. These can open a PIN prompt
on the owner's desktop.

## Phase 1 on Linux

Start every Phase 1 app through the launcher. Its Xvfb and headless Weston backends use a private display and a private, non-activating D-Bus session. Bubblewrap mounts only the system files and Flecha build files needed by the run. It gives the run private `/run` and `/var/run`, scratch XDG directories, loopback-only networking, and a files-only resolver with a localhost hosts file. The launcher clears the environment with `env -i` and gives CEF `--password-store=basic`. Keep test URLs on the fixture's loopback server.

```sh
# From rewrite/
tools/isolated-run.sh x11 -- ./target/debug/flecha
tools/verify-slice.sh
tools/verify-negative-controls.sh
```

The launcher needs a C compiler, bubblewrap, `ip`, `setpriv`, D-Bus and the selected compositor. The Xvfb verifier also needs Node, ImageMagick, and X11 and Xtst development headers. The launcher prints each scratch directory, removes it after a successful run, and retains it on failure. The watchdog uses the same default and `FLECHA_HARNESS_TIMEOUT_MS` override as the Chromium native harness. It signals the launcher by pidfd, so it also works without a delegated cgroup on CI. The kernel reaps the isolated PID namespace when its init exits.

The verifier compares live tab ordering, selection and split sides with the shipping TypeScript replay. It checks GPUI pixels before and after restart, native X11 input, CEF lifecycle events, cookies, and inherited session variables. The launcher checks for filesystem sockets and rejects public name resolution before starting its private services. The negative controls include a swapped split, a blank restart, stale callbacks, and a Peek using the shared cookie jar. Build stamps and run output are in [results/phase1](results/phase1).

Regenerate the shipping trace from the repository root with `node rewrite/tools/slice-ts-replay.mjs > rewrite/tools/slice-ts-replay.jsonl`.

The software OSR path is exercised on Xvfb. Real Wayland sessions, real GPU composition, native macOS and Windows windows, and Google sign-in remain manual checks. They require the relevant hardware, desktop session or test account.

### CEF pump and GPUI timer guard

CEF owns the 10 ms external work slice. Phase 1 keeps Spike 2's 33 ms periodic wake and arms a single cancellable timer for the earliest outstanding deadline. `Timers::arm` resets the same timer registration. CEF callbacks can replace a pending deadline and wake the pump from any thread. Frame publication happens on the GPUI thread.

The timer audit used the GPUI source at `f126950f`. `executor.rs`, `platform.rs`, `platform_scheduler.rs`, and Scheduler's `executor.rs` cover direct timers, the scheduler executor accessors, and delayed dispatch. `util.rs` wraps a timer in `FutureExt::with_timeout`. Animation frame caps, tooltip registration, image loading, and resuming a paused input sequence schedule timers in `elements/animation.rs`, `elements/div.rs`, `elements/text.rs`, `elements/img.rs` and `window.rs`. The Clippy guard covers their public entry points. Normal animation frames and SVG transformation cadence use frame requests. GPUI's own gesture, key dispatch and shutdown paths manage their internal timing.
