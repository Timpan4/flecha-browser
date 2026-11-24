# Flecha rewrite: GPUI frontend on CEF

The plan, the decisions behind it, and what each phase must prove are in [docs/cef-gpui-rewrite-plan.md](../docs/cef-gpui-rewrite-plan.md). `chromium/` is still the shipping browser. Nothing here changes it.

## Layout

- `pins.toml`: CEF, cef-rs and GPUI versions. All crates use these pins.
- `crates/<name>`: product crates (`flecha-core`, `flecha-cef`, `flecha-surface`, `flecha-ui`, `flecha-services`).
- `spikes/<name>`: Phase 0 go/no-go spikes. Each spike has a `REPORT.md` with its done check and results.

A unit adds its crate path to `members` in `Cargo.toml`.

The compositor spike uses patched copies of the pinned Zed and cef-rs sources.
From `rewrite/`, run `bash spikes/gpui-cef-compositor/prepare.sh` before resolving
the workspace with Cargo. The cef-rs build script only checks the staged
runtime and links against it. It copies nothing, so the launchers load the
runtime from `target/spike1-src/cef-runtime`. Preparation uses
`flock` to serialize staging and verifies the pinned CEF beta archive. Never run
`cargo`, `rustc`, `rustup` or Clippy inside `rewrite/target/spike1-src/zed`.
The [spike report](spikes/gpui-cef-compositor/REPORT.md) describes the runtime
and the compositor checks.

## Building on the workstation

Use plain `cargo` from `PATH`. On the workstation it is the `mbx` shim, which
shares compiled dependencies across worktrees and prunes its own storage. Run
from `rewrite/`, with Cargo's target dir left at `rewrite/target`. Dev builds
keep full debuginfo for workspace crates only. Third-party crates build without
it (`[profile.dev.package."*"]` in `Cargo.toml`).

Cargo resolves the whole workspace for every command, so a new worktree needs
the staged sources first. Reflink them from the main checkout, then let
`prepare.sh` check them against the pins and patches:

```sh
# From rewrite/ in the new worktree
mkdir -p target/spike1-src
cp -a --reflink=always ~/dev/flecha-browser/rewrite/target/spike1-src/{zed,cef-rs,cef-beta,cef-runtime} target/spike1-src/
bash spikes/gpui-cef-compositor/prepare.sh --sources-only
```

If `prepare.sh` reports that a staged tree does not match the pins or patch,
remove that copy from the new worktree's `target/spike1-src` and run
`prepare.sh --sources-only` again. It downloads and patches a fresh copy.

Crates with no CEF or GPUI dependency (`flecha-core`, `flecha-adblock`,
`flecha-import`, `flecha-migrate` and `flecha-services`) build with crate-scoped
commands and no build lock. Never use `--workspace` for them, because it
compiles CEF and GPUI too:

```sh
cargo check --locked -p flecha-core
cargo test --locked -p flecha-core
cargo clippy --locked -p flecha-core --all-targets -- -D warnings
```

The workstation runs one heavy build at a time. Every command that compiles
CEF or GPUI (`flecha`, `flecha-ui`, `flecha-cef`, `flecha-surface` and the
spikes) waits for the shared lock and runs in the agent build slice:

```sh
flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
  bash ~/.claude/skills/hosts/scripts/agent-build run cargo build --locked -j 14 -p flecha
```

If a build is killed for memory, check `free -g` before retrying.

## Desktop launch on Linux

After preparing the pinned runtime and building `flecha`, run this from
`rewrite/` in your desktop session.

```sh
tools/run-desktop.sh https://example.com
```

Use `--release` before the app arguments to select the release build, or
`--print-command` to inspect the launch command without opening a window.
The rewrite stores its profile in `$XDG_DATA_HOME/flecha-rewrite`, with
`$HOME/.local/share/flecha-rewrite` as the fallback. Use `--profile DIR` or
`FLECHA_PROFILE` to override it. Flecha rejects the shipping profile at
`~/.config/flecha-browser` and paths inside it.

A desktop check needs at least one monitor powered on. On 2026-11-24, with
every output off, COSMIC sent no Wayland frame callbacks to the rewrite. GPUI
presented only its first frame, which came before CEF painted, so the window
showed a blank page pane and window captures repeated that frame. Read
`/sys/class/drm/card*-*/dpms` before judging page rendering.

## Isolated CEF launches

Run every automated CEF launch through an isolated launcher, including tamper
and negative tests. Use Xvfb or headless Weston, a private session D-Bus with service
activation disabled, and scratch directories for `XDG_RUNTIME_DIR`, `HOME` and
`TMPDIR`. Set `DBUS_SYSTEM_BUS_ADDRESS` to a nonexistent socket path. Never use
the owner's desktop session, session bus or `/run/user/<uid>/bus`. Stop the
compositors and browsers you start by PID.

Do not run `sudo`, `pkexec`, or host system-bus or polkit probes such as
`resolvectl statistics` or `systemctl` on system units. These can open a PIN prompt
on the owner's desktop.

## Phase 1 on Linux

Use the isolated launcher for Phase 1 verification. The app requires its scratch paths for `--verify`. The Xvfb and headless Weston backends use a private display and a private, non-activating D-Bus session. Bubblewrap mounts only the system files and Flecha build files needed by the run. It gives the run private `/run` and `/var/run`, scratch XDG directories, loopback-only networking, and a files-only resolver with a localhost hosts file. The launcher clears the environment with `env -i` and gives CEF `--password-store=basic`. Keep test URLs on the fixture's loopback server.

```sh
# From rewrite/
tools/isolated-run.sh x11 -- ./target/debug/flecha
tools/verify-slice.sh
tools/verify-slice.sh focus
tools/verify-slice.sh popups
tools/verify-slice.sh clipboard
tools/verify-slice.sh tab-status
tools/verify-negative-controls.sh
```

The launcher needs a C compiler, bubblewrap, `ip`, `setpriv`, D-Bus and the selected compositor. The Xvfb verifier also needs Node, ImageMagick, and X11 and Xtst development headers. The launcher prints each scratch directory, removes it after a successful run, and retains it on failure. The watchdog uses the same default and `FLECHA_HARNESS_TIMEOUT_MS` override as the Chromium native harness. It signals the launcher by pidfd, so it also works without a delegated cgroup on CI. The kernel reaps the isolated PID namespace when its init exits.

The verifier compares live tab ordering, selection and split sides with the shipping TypeScript replay. It checks GPUI pixels before and after restart, native X11 input, CEF lifecycle events, cookies, and inherited session variables. The launcher checks for filesystem sockets and rejects public name resolution before starting its private services. The default run ends with the popup checks, which `popups` runs alone. CEF creates each browser a page opens, so `window.opener`, the referrer and the opener's cookie jar survive. The checks cover a `target=_blank` tab, a scripted popup window that messages its opener and closes itself, blocked popups without a user gesture, and closing windows with popups open. Each popup check runs on its own, so a run against an older build lists every check it fails. The negative controls include a swapped split, a blank restart, stale callbacks, and a Peek using the shared cookie jar. Build stamps and run output are in [results/phase1](results/phase1).

`clipboard` checks Copy, Cut and Paste in a page against GPUI's clipboard. With offscreen rendering, Chromium has no focused window. On Wayland the compositor serves the clipboard only to the focused client, so Chromium's own clipboard never reaches the desktop, and the shell owns page clipboard edits there. Paste commits GPUI's clipboard text as typed input. Copy and Cut read the page's selection through an internal CDP call. A Cut deletes what it read in the same page step, so no key, timer or focus change can come between the read and the delete, and the delete stays in the document the text came from. Like Chromium, Cut takes nothing from a read-only field or from page text. While a read is pending, later keys, pastes and page menu edits for that browser wait, then reach the page in their original order once the read's text is on the clipboard, so Ctrl+V right after Ctrl+X pastes the cut text even on a busy page. Input held for a browser that closes is dropped. Clicks do not wait. A password field yields nothing, and so does any selection the read cannot tie to a focused field, such as a cross-site frame or object, a closed shadow root, or a blurred field behind a focused checkbox. The held keystroke then goes to the page. The read runs in the page's own JavaScript world, so a page can change what is copied from its own document, as a `copy` handler can, but it cannot reach into a cross-site frame. The page menu's Copy uses the selection from when the menu opened. Its Cut reads and deletes the selection when chosen, and its Cut and Paste do nothing once focus has left the menu's frame, because typed input and the read go to the focused frame. Page menu Cut in a cross-site frame does nothing. Pasted text fires `beforeinput` and `input` but no `paste` event, and the clipboard carries plain text only. Elsewhere Chromium keeps page clipboard edits. Headless weston gives GPUI no input serial, so its Wayland clipboard cannot take ownership there. The check therefore runs under Xvfb, and the `shell_clipboard` verify command switches on the Wayland handling. It pastes into a text input, a password input, a textarea, a contenteditable and a cross-site frame, and asserts that Chromium fired no `paste` event. It copies and cuts from inputs, a contenteditable and page text, both from the keyboard and from the page menu. It also checks that no path copies or cuts a password. The fixture cancels Ctrl+C and Ctrl+X keydowns, so Chromium never takes the X11 clipboard from GPUI. The last checks race a cut against the page: keys typed right after Ctrl+X, a page moving focus between two selections, a `beforeinput` handler moving the selection, a top page forging the read over a password in a cross-site frame, a selection change while the page menu is open, and focus moving into another frame before menu Paste. Three more press Ctrl+X followed by typing, by Ctrl+V, or by End and Ctrl+V while the page runs a long task, and expect Chromium's order of edits. Each reports its own failure. A real Wayland session remains a manual check.

`tab-status` checks the tab row's status marks. A page declares an icon in its `<head>`, and the row draws the downloaded image. A held response keeps a page loading, and its row shows the spinner until the response arrives. A page starts an oscillator from an X11 click, CEF reports it audible, and clicking the row's speaker mutes the page in CEF and in the saved tab, then unmutes it.

Regenerate the shipping trace from the repository root with `node rewrite/tools/slice-ts-replay.mjs > rewrite/tools/slice-ts-replay.jsonl`.

The software OSR path is exercised on Xvfb. Real Wayland sessions, native macOS and Windows windows, and Google sign-in remain manual checks. They require the relevant hardware, desktop session or test account.

### Page frames on the GPU

On Linux, CEF hands page frames to GPUI as shared GPU textures (`OnAcceleratedPaint`, a dma-buf) when it can. GPUI picks its GPU when it configures a window's surface, and Chromium starts its GPU process inside `CefInitialize`, so Flecha opens an empty first window before CEF starts, and the first main window takes it over (`FirstWindow` in `flecha-ui`). If Chromium's `SingletonLock` in the profile names a live process on this host, CEF will forward the launch, so Flecha opens no window and starts CEF directly. If that process exits before CEF checks the lock, this Flecha runs with CPU copies. Flecha reads the DRM render node of GPUI's Vulkan adapter and passes it to Chromium as `--render-node-override`, so on a host with several GPUs CEF allocates on the GPU that GPUI draws with. On X11, Chromium's DRI3 device must be that GPU too. If the adapter lacks dma-buf import or `VK_EXT_physical_device_drm`, or the X server has no matching DRI3 device, every browser gets CPU copies through `OnPaint`. Under Xvfb, GPUI's adapter has no Vulkan dma-buf import, so the Xvfb harnesses always use CPU copies. The DRI3 check has not run against an X server that has a DRI3 device.

CEF fixes a browser's mode when it creates it. If a shared texture cannot be imported, the engine discards that browser and reopens its page with CPU copies. The engine keeps the browser's last page frame and shows it until the new browser paints. Reopening loses what a discarded tab loses: back and forward history, form state and `window.opener`. Browsers created after that also get CPU copies. If GPUI recreates its device on the same GPU, browsers repaint for it. If GPUI moves to another GPU, browsers that share textures reopen with CPU copies. Each change logs one `page frames:` line on stderr.

`tools/verify-gpu-frames.sh` checks the shared-texture path under the isolated launcher's headless weston. It needs a GPU and runs on the workstation, not CI. It checks a page, a `<select>` popup, split view, Peek, a window resize and a scripted popup window by counting the fixture marker's pixels. It also checks that a second launch with `--open` opens no window, exits 0, and that its address paints from shared textures in the running Flecha. `tools/verify-gpu-frames.sh fallback` opens two pages in split view that share textures. It then sends the `reject_next_shared_texture` verify command and repaints the left page, so that page's next import fails. The check confirms three things:

- the left page reopens with CPU copies
- the right page keeps sharing and repainting
- the next new browser gets CPU copies

### CEF pump and GPUI timer guard

CEF owns the 10 ms external work slice. Phase 1 keeps Spike 2's 33 ms periodic wake and arms a single cancellable timer for the earliest outstanding deadline. `Timers::arm` resets the same timer registration. CEF callbacks can replace a pending deadline and wake the pump from any thread. Frame publication happens on the GPUI thread.

The timer audit used the GPUI source at `f126950f`. `executor.rs`, `platform.rs`, `platform_scheduler.rs`, and Scheduler's `executor.rs` cover direct timers, the scheduler executor accessors, and delayed dispatch. `util.rs` wraps a timer in `FutureExt::with_timeout`. Animation frame caps, tooltip registration, image loading, and resuming a paused input sequence schedule timers in `elements/animation.rs`, `elements/div.rs`, `elements/text.rs`, `elements/img.rs` and `window.rs`. The Clippy guard covers their public entry points. Normal animation frames and SVG transformation cadence use frame requests. GPUI's own gesture, key dispatch and shutdown paths manage their internal timing.
