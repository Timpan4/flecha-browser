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
  bash ~/.claude/skills/hosts/scripts/agent-build run ~/.cargo/bin/cargo build -j 14
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
