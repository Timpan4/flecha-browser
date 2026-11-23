# Spike 0: Alloy OSR sign-in and Chrome features

Plan: [docs/cef-gpui-rewrite-plan.md](../../../docs/cef-gpui-rewrite-plan.md), section "Spike 0".

## Status

- Built on the pins against the **beta** CEF archive (SHA-1 checked) in a fresh clone, and self-checked in both modes on Xvfb. These also passed on Xvfb, every one as a failing assertion when wrong:
  - OSR input, including Swedish AltGr and French AZERTY;
  - the `window.open` popup and a notification permission prompt;
  - printing to PDF and the terminal commands;
  - console relay off for web pages;
  - restart persistence;
  - Chrome-mode tab handling.

  Results are in [Automated results](#automated-results).
- Checks 0 and 4 to 7 of the sign-in gate now run automatically on Xvfb in both modes and in today's Flecha (`scripts/auto-checks.sh`, see [Checklist rows on Xvfb](#checklist-rows-on-xvfb)). In OSR, typing, the camera prompt, print and password fill pass. The Web Store install crashes the browser process or installs nothing, the content script did not run in the recorded runs, no password save is offered, and paste from the X clipboard fails. The Chrome-style control passes everything except the extension popup (4c), which needs Chrome's own toolbar.
- Checks 1 to 3, Google sign-in, are **not run**. They need the owner and a purpose-made Google account. The shortened checklist is in [Manual checklist](#manual-checklist-for-the-owner).
- Nothing in the automated results blocks the manual sign-in run. Both modes present the same identity to pages: `navigator.webdriver` false, CEF's default user agent, `Chromium` and `Not.A/Brand` brands with no `Google Chrome` brand. Both run fully sandboxed. The Web Store offers "Add to Chrome" in both modes, next to an "Install Chrome" button (see [Web Store](#web-store)).

## What the app is

One binary, `cef-osr-signin`, with two modes:

- `--mode osr`: an Alloy-style windowless browser. CEF paints BGRA frames through `OnPaint`, and the app copies them into a winit window with softbuffer. Mouse, wheel and keyboard go to CEF through `BrowserHost::send_*_event`. `<select>` popups (`PET_POPUP`) are drawn over the page. Every `window.open` or `target=_blank` popup gets its own windowless browser and its own window, so OAuth popups keep their opener. Camera, microphone and other permission requests are asked in the terminal. The CEF message pump runs on the winit thread through `OnScheduleMessagePumpWork`.
- `--mode chrome`: the control. `CefBrowserHost::CreateBrowser` with Chrome style and no parent window, so CEF opens a normal Chrome window with Chrome's tab strip, address bar, extension menu and profile button.

Both modes use the same settings except the windowing ones (`windowless_rendering_enabled` and `external_message_pump` are on only in OSR mode):

- Sandbox on (`no_sandbox = 0`).
- No `--remote-debugging-port` or pipe, no automation switches. The program passes CEF only `argv[0]` plus anything after `--`; its own flags never reach CEF.
- CEF's default user agent. `user_agent` and `user_agent_product` are unset.
- `root_cache_path` and `cache_path` are both `--profile`, which must be an absolute path. The profile persists across runs.
- Background colour opaque white in both modes (without it, OSR pages with no background paint transparent, which showed as black).
- `persist_session_cookies` is left at CEF's default (off), as in Chrome.

## Pins and what actually loaded

| Item | Pin (`rewrite/pins.toml`) | Observed |
|---|---|---|
| cef-rs | `Timpan4/cef-rs` rev `32d6bc55` | Built from that rev, as Spike 1's `prepare.sh` stages it (`.flecha-revision` names the rev) with `spikes/gpui-cef-compositor/patches/cef-rs.patch` applied. The patch changes cef-dll-sys's build script and `Cargo.toml`, adds a compiled-in archive SHA-1 constant, and changes the `accelerated_osr` texture import code, which this spike does not enable (`cargo tree -e features`: `cef` with `sandbox` only) |
| CEF version | `158.0.3+g8c29be2+chromium-158.0.8127.18` | `chrome://version`: `CEF 158.0.3+g8c29be2+chromium-158.0.8127.18`, `Chromium 158.0.8127.18 (Official Build)` |
| CEF commit | `g8c29be2` | `cef_api_hash(…, 2)` at run time: `8c29be240971eebee960ac92114d3efccb5780ff` |
| Archive | beta SHA-1 `057a0b87…` | `cef_binary_158.0.3+g8c29be2+chromium-158.0.8127.18_linux64_beta_minimal.tar.bz2`, SHA-1 `057a0b871bf26d26d654d999dba595b92fe0f067`, checked by `prepare.sh` before staging and by the binary at start (`archive.json` next to the loaded `libcef.so`). The beta gap of the first round is closed for this spike. |
| CEF API version | `CEF_API_VERSION_LAST` 15800 in the bindings | `cef_api_version()` at run time: 15800 (checked at start) |

## Build and run

Run from `rewrite/`. Three steps:

```sh
# 1. Download the pinned CEF archive, check its SHA-1 against rewrite/pins.toml, and stage it at
#    target/cef-runtime/158.0.3/cef_linux_x86_64 (a 324 MB download, 1.6 GB staged).
bash spikes/cef-osr-signin/prepare.sh
#    Stage the workspace's cef-rs source (and Spike 1's own copy of the same archive).
bash spikes/gpui-cef-compositor/prepare.sh

# 2. Build through the shared lock (see rewrite/README.md).
flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
  bash ~/.claude/skills/hosts/scripts/agent-build run bash spikes/cef-osr-signin/build.sh -j 14

# 3. Start through the launcher (examples below).
```

How the runtime is found:

- Both spikes are members of the `rewrite/` workspace, and a workspace can hold only one cef-dll-sys, because it declares `links = "cef_dll_wrapper"`. So this spike uses the workspace's `cef` dependency: Spike 1's staged copy of the same pinned cef-rs rev, with Spike 1's patch. That patched build script ignores `CEF_PATH`. It checks Spike 1's staged copy of the same pinned beta archive against `pins.toml`, links against it, and copies it into `target/debug`. A build through the workstation's caching `cargo` can skip that copy: the fresh clone had none, the working tree had one.
- `build.sh` checks this spike's staged runtime and Spike 1's staged cef-rs source, then runs `cargo build --locked -p cef-osr-signin`.
- The binary has no RUNPATH, so the dynamic loader never looks in `target/debug`, even with Spike 1's copy there. `run.sh` checks this spike's staged runtime and starts the binary with `LD_LIBRARY_PATH` set to it. Started any other way, the loader fails before `main` (`libcef.so: cannot open shared object file`). The self-check report's `cef.loaded_libcef` names the copy that loaded. In a run with a full runtime copy in `target/debug`, all 10 app processes mapped `libcef.so`, the paks, ICU data and locales from this spike's stage, and none mapped or held open a file under `target/debug` (other than the binary) or Spike 1's stage.
- At start, before any CEF call, `main` checks that the loaded runtime is the pinned one. It checks the API version (15800), the commit (`8c29be2`), the `archive.json` SHA-1 next to the loaded `libcef.so` (the `pins.toml` value, compiled in by `build.rs`), and the resource files. If any check fails, it exits with `wrong CEF runtime: …`. It points CEF's `resources_dir_path` and `locales_dir_path` at that directory.

Run with a new, dedicated profile directory. Never point `--profile` at a real browser profile or at `.flecha-profile`.

```sh
# Alloy OSR, through XWayland (the tested path)
env -u WAYLAND_DISPLAY spikes/cef-osr-signin/run.sh --mode osr \
  --profile "$HOME/flecha-spike0/osr" --url https://accounts.google.com/

# Chrome-style control, through XWayland
env -u WAYLAND_DISPLAY spikes/cef-osr-signin/run.sh --mode chrome \
  --profile "$HOME/flecha-spike0/chrome" --url https://accounts.google.com/

# Native Wayland (untested; see below)
spikes/cef-osr-signin/run.sh --mode osr --profile "$HOME/flecha-spike0/osr-wayland" \
  --url https://accounts.google.com/ -- --ozone-platform=wayland
```

`env -u WAYLAND_DISPLAY` makes winit and CEF both use X11 (XWayland on a Wayland desktop). Every automated run was on Xvfb, so that is the tested path. For native Wayland, winit picks Wayland when `WAYLAND_DISPLAY` is set, and CEF needs `--ozone-platform=wayland`, because otherwise it adds `--ozone-platform=x11` itself. On Xvfb, with no compositor, that switch made CEF try the Wayland platform (`Failed to connect to Wayland display`). So the switch is honoured, but nothing ran on a Wayland compositor: the Xvfb-only rule ruled it out. Use a separate profile for it. If it fails, use the XWayland commands.

#### Console output and logs

The terminal shows load events, popups and permission requests. Paths, queries, fragments and opaque URL payloads can carry OAuth codes or account identifiers, so every URL the app prints goes through one function (`display_url` in `src/common.rs`) that keeps only:

- `scheme://host[:port]` for `http`, `https`, `ws`, `wss`, `ftp` and `chrome-extension` URLs, without any `user:password@` (`https://accounts.google.com`);
- the path as well for `file://`, `chrome://` and `devtools://` URLs, still without query or fragment;
- only `scheme:` for every other scheme, whatever follows the colon (`data:`, `mailto:`, `javascript:`, `blob:`, `about:`, also `data://…`).

The scheme is compared in lower case. A `\` counts as `/` only for the special schemes of the WHATWG URL standard (`http`, `https`, `ws`, `wss`, `ftp`, `file`), as browsers parse them; for every scheme a `\` ends the host.

Page `console.log` output is relayed only from the spike's local fixtures. CEF's `OnConsoleMessage` does not say which frame logged a message, and a page can set a message's source to any URL with `//# sourceURL`. So a message is relayed only when its source is a `file://` URL and every frame in the browser, the main frame and each iframe, is a `file://` page. A local page that embeds any remote frame relays nothing, not even its own lines. `--log-console` relays every page's console output; do not use it during the sign-in checks.

CEF also keeps its own log, `chrome_debug.log` in the profile directory. By default Chromium writes every page console message that no handler consumed to it at INFO level, with the full source URL. The app consumes every console message (`on_console_message` returns true), and without `--log-console` it sets CEF's log severity to WARNING, so neither console output nor INFO lines reach that file. The action scripts check that the marker page's console line is absent from it in both modes.

#### Keyring or basic password store (an owner choice for checks 1 and 5)

Chromium picks a password store from the desktop environment and falls back to `basic` when that store is unavailable ([Chromium docs/linux/password_storage.md](https://chromium.googlesource.com/chromium/src/+/main/docs/linux/password_storage.md)). Choose before check 1 and use the same choice for every run on that profile:

- **Default (your login keyring).** Closest to a shipping browser. The browser will likely store its key in your GNOME keyring (libsecret), adding an entry there and possibly showing an unlock prompt. You can delete that entry after the test.
- **`-- --password-store=basic`.** Nothing touches your keyring. Passwords are stored without encryption ("Passwords stored in plain text are not encrypted", same doc) in the test profile, so delete the profile after the test. It tests less of the shipping path.

Both switches are allowed after `--`; switches that change the browser's identity or security (the forbidden list under [Self-check](#self-check), and `--type`) are refused there.

OSR mode has no address bar. Type commands in the terminal it was started from:

| Command | Effect |
|---|---|
| `go <url>` | Load a URL in the focused window |
| `back`, `forward`, `reload` | Navigation |
| `print` (or Ctrl+P in the window) | `BrowserHost::Print()` |
| `pdf [path]` | `BrowserHost::PrintToPDF()`, default `<profile>/flecha-spike-print.pdf` |
| `allow <id>`, `deny <id>` | Answer a permission request the terminal printed |
| `quit` | Close all browsers and exit cleanly |

Close the window (or type `quit`) to exit. The app closes every browser and waits for `OnBeforeClose` before `CefShutdown`, so cookies are flushed. Do not kill it if you are testing restart persistence.

Self-check (prints one JSON report and exits):

```sh
spikes/cef-osr-signin/run.sh --mode osr --profile /tmp/spike0-check/osr \
  --self-check --self-check-timeout-secs 30 --report /tmp/spike0-check/osr.json
```

`--self-check-timeout-secs` is required. It bounds each step; 30 is the value used for the runs below, not a product limit.

The self-check exits with status 1, and prints `self-check failed: …` lines, when `chrome://sandbox` does not say "You are adequately sandboxed", a forbidden switch is on any process, `navigator.webdriver` is not `false`, or the process scan was incomplete. It also exits with status 1 when it cannot write the `--report` file. The scan reads each process's `/proc` name and status as bytes and decodes them lossily, so an unrelated process with a non-UTF-8 name does not make it incomplete; only read errors do. The report's `pass` and `failures` fields say the same. Switches are read as Chromium reads them on Linux: `--name` and `-name` are the same switch (`kSwitchPrefixes` in `base/command_line.cc`). The refusal after `--`, the forbidden-switch audit and the report all use that rule.

The report redacts command lines: each process's switches come from its argv (`/proc/<pid>/cmdline`), names only, with values kept for a short allowlist such as `--type` and `--lang` and only when they contain no path characters. The `Command Line` row of `chrome://version` is dropped, since it joins arguments with spaces. Home directories become `~` in every string, including percent-encoded `file://` URLs.

To run anything on Xvfb with an isolated session, use `WAIT_SECS=<secs> spikes/cef-osr-signin/scripts/xvfb-isolated.sh <out-dir> <app args…>`. It gives everything it starts a scratch HOME, XDG and runtime dirs, no session bus address, a system bus address that points at a path that does not exist (`DBUS_SYSTEM_BUS_ADDRESS`), no keyring or desktop-session variables and no `WAYLAND_DISPLAY`. The app also gets a private D-Bus bus without service activation, and is started through `run.sh`. `WAIT_SECS` is required. It bounds Xvfb readiness, the first page load, every step of an action script, and the SIGTERM grace before SIGKILL. The runs used 30; it bounds a test run and is not a product limit. The app's stdin is the FIFO `<out-dir>/stdin.fifo`, so OSR commands can be sent to it. With `ACTIONS=<script>`, the harness:

1. waits for the first page load;
2. runs the script with `DISPLAY`, `OUT`, `APP_PID` and `CMD` set;
3. fails if the script fails;
4. returns the app's exit status if the script ended the app.

Xvfb and the app run in their own process groups, and the harness stops those groups. The action scripts need:

- `XDOTOOL`: path to xdotool. It is not installed on the workstation; the runs used nixpkgs' build (`nix --extra-experimental-features 'nix-command flakes' build nixpkgs#xdotool`).
- `node`: `scripts/fixture-server.mjs` serves loopback pages (a page that logs two console markers naming its case, one plain and one with its source spoofed to `file://` through `//# sourceURL`, sets a cookie and then requests `/ran`; and a plain page), so no check needs the network. It logs every request; the scripts wait for the marker page's `/ran` request before asserting that its console line was not relayed, so a page that never loaded or never ran fails the check.
- `WM` (only `persist-actions.sh`): an EWMH window manager binary. Xvfb has none, and closing a window the way a desktop does needs one. The runs used nixpkgs' openbox 3.6.1.
- `EXPECT_MARKER` (only `console-actions.sh`): 0 for a default run, 1 with `--log-console`.
- `PROFILE` (optional): the app's `--profile` directory. `osr-actions.sh`, `chrome-actions.sh` and `console-actions.sh` read `chrome_debug.log` there; the default is `<out-dir>/profile`.

Example, from `rewrite/`:

```sh
ACTIONS=$PWD/spikes/cef-osr-signin/scripts/osr-actions.sh WAIT_SECS=30 XDOTOOL=/nix/store/…-xdotool-…/bin/xdotool \
  bash spikes/cef-osr-signin/scripts/xvfb-isolated.sh /tmp/spike0-osr --mode osr --profile /tmp/spike0-osr/profile \
  --url "file://$PWD/spikes/cef-osr-signin/scripts/input-test.html"
```

### Workarounds in this spike

- `rewrite/.cargo/config.toml` sets `net.git-fetch-with-cli = true`. With the user's `url.insteadOf` SSH rewrite, Cargo's built-in git client cannot fetch the workspace's pinned git dependencies: with an empty `CARGO_HOME` and the setting off, it failed with `revision 3759371fee14c40066d64777e2a36b6ffcc22590 not found` for the zed calloop fork. The git CLI fetches them.
- The CEF runtime this spike loads is staged by its own `prepare.sh`, not by a build script. At the pinned rev, upstream cef-dll-sys copies the runtime to `OUT_DIR`'s third parent, which under Cargo 1.100's build-dir layout is `target/debug/build`, not `target/debug` ([tauri-apps/cef-rs#522](https://github.com/tauri-apps/cef-rs/pull/522), open). Since the merge with Spike 1, the workspace builds Spike 1's patched cef-rs, which links against Spike 1's own stage and copies it into `target/debug`. Both stages are the same pinned beta archive, SHA-1 checked by each prepare step, so the build links against the same files the binary loads; the binary loads only this spike's stage, through `run.sh`. One shared prepare step should replace both.

## Automated results

Run on 2026-11-23 on the workstation, in a fresh `git clone` of the branch, built with the [Build and run](#build-and-run) commands through the `cargo` on PATH. Everything ran on Xvfb only, through `scripts/xvfb-isolated.sh`, with a fresh profile per run (the second persistence run reuses the first run's profile) and `WAIT_SECS=30`. The self-checks used `--self-check-timeout-secs 30` and a profile path containing `#`, `%` and a space. The JSON reports and logs are in [evidence/](evidence/), with scratch paths shortened to `…`.

### Runtime checks

Command output: [evidence/runtime-checks.txt](evidence/runtime-checks.txt).

| Check | Result |
|---|---|
| `run.sh` before `prepare.sh` | exit 1: `No CEF runtime at …/target/cef-runtime/158.0.3/cef_linux_x86_64. Run spikes/cef-osr-signin/prepare.sh first.` |
| `prepare.sh` | downloaded the beta archive, `sha1sum --check` passed, staged it; no `download.*` or `unpack.*` left |
| `prepare.sh` with `include/` moved out of the stage | exit 1: `The CEF runtime at … is incomplete: include is missing. Remove the directory and run prepare.sh again.` |
| `prepare.sh` with the archive removed and a `curl` that writes junk | exit 1: `sha1sum` reports the mismatch; no `download.*` file left |
| `run.sh` with `FLECHA_SPIKE_BIN=no-such-spike-binary` | exit 1: the bare name is resolved through `PATH` before the check |
| `build.sh` before Spike 1's `prepare.sh` | exit 1: `No staged cef-rs source at …/target/spike1-src/cef-rs. Run spikes/gpui-cef-compositor/prepare.sh first.` |
| Spike 1's `prepare.sh` | exit 0; its stage's `libcef.so` has the same SHA-1 as this spike's |
| `build.sh` through the lock, PATH `cargo` (the caching `mbx` shim) | exit 0; `Cargo.lock` unchanged; `readelf -d`: `NEEDED libcef.so`, no RUNPATH. In this clone no runtime was copied to `target/debug`; where one is, it is on no library search path and is never loaded (see [Build and run](#build-and-run)) |
| The binary started without `run.sh` | exit 127: `libcef.so: cannot open shared object file` (dynamic loader) |
| `libcef.so` copied alone into a directory, `LD_LIBRARY_PATH` pointing there | exit 1: `wrong CEF runtime: cannot read …/archive.json` |
| Self-check, both modes | loaded `target/cef-runtime/158.0.3/cef_linux_x86_64/libcef.so`, API version 15800, commit `8c29be24…`, archive `cef_binary_158.0.3+g8c29be2+chromium-158.0.8127.18_linux64_beta_minimal.tar.bz2`, SHA-1 `057a0b87…` |

### URL display probe

The second verifier's probe, run with default flags against this clone's build ([log](evidence/url-probe.txt)). A local page opened 9 popups through `window.open` with test tokens in opaque and hierarchical URLs (`data:`, `data://…`, `mailto:`, userinfo, query and fragment, `javascript:`, `javascript://…`, `blob:`, `about:`). It also embedded a remote iframe that logs console lines with its source spoofed to `file://`. In OSR mode `go` then loaded 10 more URLs, including `data://…` and `data:\\…`. Every `[popup]`, `[load-end]` and `[load-error]` line printed `scheme:` or an origin, and no test token reached stderr, stdout or `chrome_debug.log` in either mode. The only token-like text is `zqxhost29.invalid`, the host of an `https` URL, which the rule keeps. In Chrome mode the probe's fixed-position clicks opened no popups, so popup coverage is OSR only; the spoofed iframe ran and relayed nothing in both modes.

### Self-check

| Field | Alloy OSR | Chrome-style control |
|---|---|---|
| Runtime style (`BrowserHost::GetRuntimeStyle`) | alloy, windowless | chrome, windowed |
| `navigator.webdriver` | `false` | `false` |
| `navigator.userAgent` | `Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/158.0.0.0 Safari/537.36` | same |
| `navigator.userAgentData.brands` | `Not.A/Brand` 24, `Chromium` 158 | same |
| `fullVersionList` (high-entropy) | `Chromium` 158.0.8127.18 | same |
| `window.chrome` keys | `loadTimes`, `csi`, `app` | same |
| Screen seen by the page | 1280×1024, colour depth 24 (from the Xvfb screen through `GetScreenInfo`) | 1280×1024, depth 24 |
| `chrome://version` CEF / Chromium | `158.0.3+g8c29be2+chromium-158.0.8127.18` / `158.0.8127.18 (Official Build)` | same |
| Runtime CEF commit (`cef_api_hash(…, 2)`) | `8c29be24…` | same |
| `chrome://sandbox` | Namespace layer-1 sandbox, PID and network namespaces, seccomp-BPF with TSYNC: "You are adequately sandboxed." | same |
| Self-check verdict (`pass`, exit status) | `true`, 0 | `true`, 0 |
| /proc: renderer-type processes | seccomp mode 2, `NoNewPrivs` 1, own user, PID and network namespaces | same |
| /proc: processes without seccomp | browser, network service (`--utility-sub-type=network.mojom.NetworkService`), zygote helpers; Chromium's normal Linux layout. One zygote runs as `--type=zygote --no-zygote-sandbox`: Chromium starts it that way for processes that run unsandboxed, so the audit accepts that switch on a `--type=zygote` process only | same |
| Forbidden switches in any process command line (`--remote-debugging-*`, `--enable-automation`, `--headless`, `--no-sandbox`, `--disable-gpu-sandbox`, `--no-zygote-sandbox` and other sandbox-off switches, `--user-agent`, `--disable-web-security`, `--ignore-certificate-errors`, `--use-mock-keychain`, `--test-type`, fake media devices) | none; process scan complete, no read errors. Rerun on this branch's binary with the longer list: both modes pass | same |
| Profile path with `#`, `%20` and a space | the page step loaded `…/pro%23file%20%2520/flecha-selfcheck.html` (the percent-encoded URL). The report shows it decoded, `…/pro#file %20/flecha-selfcheck.html`, because it contained a home path that redaction replaced | same |

Switches on the browser process (from `chrome://version`, identical in both modes): `--lang=en-US --resources-dir-path=… --locales-dir-path=… --disable-signin-promo-on-avatar-pill-for-testing --disable-features=GlicActorUi,AutofillActorMode,LensOverlay,KillOnInvalidNavigationHeaders --ozone-platform=x11`. CEF adds them itself; the two `-dir-path` switches come from the app's `resources_dir_path` and `locales_dir_path` settings. None of them is visible to pages. Judging only by its name, `--disable-signin-promo-on-avatar-pill-for-testing` hides a sign-in promo on the profile avatar button; I did not check Chromium's source for what else it does.

### Web Store

Page: uBlock Origin Lite, `https://chromewebstore.google.com/detail/ublock-origin-lite/ddkjiahejlhfcafbddmgiahcphecmpfh`. Nothing was clicked except "Reject all" on Google's consent page, which the store redirects to from a Swedish address (`consent.google.com`; the report records only that the click happened). Nothing was installed and no account was used.

| Field | Alloy OSR | Chrome-style control |
|---|---|---|
| Final page | the detail page, after the consent redirect | same |
| "Add to Chrome" | present, enabled | present, enabled |
| Other offer | "Install Chrome" button, enabled | same |
| `typeof chrome.webstorePrivate` | `object` | `object` |
| `typeof chrome.management` | `object` | `object` |

"Add to Chrome" first renders disabled and turns enabled once the page has finished its own checks. An earlier run that reported at first render saw it disabled; the self-check now waits for the enabled state. The "Install Chrome" button suggests the store does not take either mode for Google Chrome (no `Google Chrome` brand in `userAgentData`). Clicking "Add to Chrome" is now automated with Dark Reader (check 4a in [Checklist rows on Xvfb](#checklist-rows-on-xvfb)): it installs in the Chrome-style control and crashes or installs nothing in OSR. In OSR there is no extension toolbar, so even a successful install has no surface for the popup.

### OSR input on Xvfb

`scripts/input-actions.sh` drives `scripts/input-test.html` in an OSR window with xdotool and fails unless the page reports each expected event. Before typing into a field it clicks the field and waits for the page's `focus` event, so keys cannot go to the document. It ran 6 times in the fresh clone and passed 6 times ([per-run summary](evidence/input-runs.txt)). That was one sample: issue #94 measured 8 of 10, and on this branch it passed 14 of 15 (12 of 12 on the final binary) (see the input finding under [Checklist rows on Xvfb](#checklist-rows-on-xvfb)). The page logs what it receives ([full log](evidence/osr-input-events.txt)):

| Action | Page saw |
|---|---|
| Click a text field, type `Hello Wörld 123!` | value `Hello Wörld 123!` |
| Click a password field, type the scripted test string `pa$$ wörd`, Enter | `password length 9 scripted true` (the page never logs a password value or the password field's key presses) |
| Textarea: `line1`, Enter, `line2`, Ctrl+A, Backspace, `kept` | `line1\nline2`, then empty, then `kept` |
| Double click a button | `click` detail 1, `click` detail 2, `dblclick` |
| Click a `<select>`, Down, Enter | popup drawn over the page ([screenshot](evidence/osr-select-popup.png)), `change` to `two` |
| Three wheel notches down | `scroll` events with a positive `scrollY` |
| Ctrl+P | `[print] start` from the app's print handler (CEF's `OnPrintStart`), then Chromium's "The selected printer is not available" error, since the handler supplies no printer settings and Xvfb has no printers; the app is still running |

Screenshots: [OSR page after the input run](evidence/osr-page.png), [Chrome-style control with two tabs](evidence/chrome-two-tabs.png).

### Behaviour checks on Xvfb

Each row is one scripted run through `scripts/xvfb-isolated.sh` on `scripts/input-test.html`, with `WAIT_SECS=30`. Every row except the camera row is an assertion: a wrong outcome fails the script and the run. The camera row records whether a prompt appeared and asserts only that a denied prompt is refused. Console checks first wait for the marker page's `/ran` request, so they fail if the page did not load or run. The navigation and cookie checks use the loopback fixture server, not the network. Logs: [OSR](evidence/osr-actions.txt), [Chrome](evidence/chrome-actions.txt), [OSR restart](evidence/osr-persist.txt), [Chrome restart](evidence/chrome-persist.txt), [console default](evidence/console-default.txt), [console with `--log-console`](evidence/console-log-console.txt).

| Check | Script | Result |
|---|---|---|
| OSR `window.open` popup | `osr-actions.sh` | A second OSR window opened and painted ([screenshot](evidence/osr-popup.png)); the popup page saw `window.opener` non-null; `window.close()` in the popup closed only the popup (open browsers 2, then 1) and the app kept running |
| OSR notification permission | `osr-actions.sh` | `Notification.requestPermission()` printed `[permission 1] file:/// requests permission types 0x8000`; `allow 1` on stdin resolved it as `granted` (a `deny` makes the script fail) |
| OSR camera permission | `osr-actions.sh` | `getUserMedia({video: true})` failed with `NotFoundError` and **no prompt**: Xvfb has no camera, and the request ends before any permission callback. With a fake camera the prompt path does run: see check 6 in [Checklist rows on Xvfb](#checklist-rows-on-xvfb) |
| OSR AltGr on a Swedish layout | `osr-actions.sh` | With `setxkbmap se`, AltGr+2 then `åäö` gave the text field `@åäö` |
| OSR Ctrl+A on French AZERTY | `osr-actions.sh` | With `setxkbmap fr`, the page saw `keydown a KeyQ 65 ctrl true` (VK_A from the layout, physical position Q, Ctrl held; a plain `a` would also be `KeyQ 65`, so the Ctrl flag is asserted too). Before the fix it saw `keydown q KeyQ 81`, so Ctrl+A acted as Ctrl+Q |
| OSR `pdf <path>` | `osr-actions.sh` | `[pdf] ok=true`, file not empty |
| OSR `go <url>` | `osr-actions.sh` | `[load-end] http://127.0.0.1:<port>`, logged after the command |
| OSR console relay, default | `osr-actions.sh`, `console-actions.sh` with `EXPECT_MARKER=0` | The loopback marker page loaded and ran (its `/ran` request arrived), as the main frame and as a remote iframe inside a local `file://` page. Each load logs a plain marker and one whose source is spoofed to `file:///flecha-spoof.js`. Each of the four was counted on its own: 0 lines in stderr, while `input-test.html`'s own console lines were relayed. 0 marker lines in the profile's `chrome_debug.log`. With the previous rule (a `file://` source and a `file://` main frame), this run failed on the spoofed iframe marker; with a rule that trusts any `file://` source, on the spoofed main-frame marker |
| OSR console relay, `--log-console` | `console-actions.sh` with `EXPECT_MARKER=1` | 1 line for each of the four markers (plain and spoofed, main frame and iframe), counted on its own: the check can see each one. 0 marker lines in `chrome_debug.log` |
| OSR `quit` | `osr-actions.sh` | app exit status 0 |
| Chrome: tab opened from Chrome's UI | `chrome-actions.sh` | Ctrl+T, then the loopback marker page through the omnibox: the new tab got the app's client (`[browser] created id=2 open=2`, `[load-end] http://127.0.0.1:<port>`) ([screenshot](evidence/chrome-two-tabs.png)); 0 marker lines in stderr and in `chrome_debug.log` |
| Chrome: close the first tab | `chrome-actions.sh` | Ctrl+1, Ctrl+W: browser 1 closed, the app kept running with one tab |
| Chrome: close the last tab | `chrome-actions.sh` | Ctrl+W: app exit status 0 |
| Restart persistence, OSR | `osr-actions.sh` then `persist-actions.sh` | Second run on the same profile: localStorage load counter 3 (the first run loaded the page twice); `Default/Cookies` has the fixture's `flecha_fixture` cookie for `127.0.0.1`; closing the window through the window manager exited with status 0 |
| Restart persistence, Chrome | `chrome-actions.sh` then `persist-actions.sh` | Counter 2, fixture cookie present, window close exited with status 0 |

The first run on each profile ended with `quit` (OSR) or by closing the last tab (Chrome); the second ended with a window close. Restart persistence of a Google sign-in is still manual check 1.

### Checklist rows on Xvfb

`scripts/auto-checks.sh` runs checklist checks 0 and 4 to 7 in Alloy OSR, the Chrome-style control and today's Flecha, and writes a table in the [Results](#results-fill-in-pass-fail-or-na-plus-a-short-note) shape with a build stamp. Checks 1 to 3 need a Google account and are never automated. Two runs on the same commit and binary (`7538752`, a clean tree; later commits add only results and REPORT text). Rows 1 to 3 say `manual` there instead of staying blank, so a reader cannot take a blank cell for a skipped check; the owner's cells are in the table under [Manual checklist](#manual-checklist-for-the-owner): [results/auto-checks/run1.md](results/auto-checks/run1.md) and [results/auto-checks/run2.md](results/auto-checks/run2.md), with each case's log in `results/auto-checks/run1-evidence/` and `run2-evidence/`. The two runs agree on every cell except the counts of key presses. Both exited 0: no control passed, every guard row passed, no row stayed unobserved, no process group survived and the scratch directory was removed.

Over the branch's 9 full runs, every row had the same outcome in every run, with three exceptions. Once the OSR install installed nothing instead of crashing. Two harness races, fixed and described under Findings, failed the OSR guard's control once and Flecha's camera step once.

```sh
# From rewrite/, after Build and run. TMPDIR must be short: Chromium's singleton socket path
# has to fit in 107 bytes (unix(7)), and xvfb-isolated.sh refuses a longer one.
TMPDIR=$XDG_RUNTIME_DIR WAIT_SECS=30 XDOTOOL=/nix/store/…-xdotool-…/bin/xdotool XCLIP=/nix/store/…-xclip-…/bin/xclip \
  bash spikes/cef-osr-signin/scripts/auto-checks.sh /tmp/spike0-auto.md osr chrome flecha
```

Every launch goes through `xvfb-isolated.sh` (its own Xvfb, a private session bus with activation off, a system bus address that points nowhere, scratch HOME, XDG, runtime and TMPDIR dirs, a scratch profile) and passes `--password-store=basic`. Each pass condition also runs as a negative control with its cause removed. The runner exits 1 if a control passes, a guard row does not pass, a row stays inconclusive, a process group it started is still alive, or its scratch directory remains.

| Check | How it is observed | Negative control |
|---|---|---|
| 0, typing | `setxkbmap se` on the Xvfb display; xdotool clicks the field, waits for its `focus` event and types `@`, `åäö`, AltGr characters, `€` and a Shift capital; the page's `[console] INPUT text` line must match exactly | no click into the field, so the keys go to the document |
| 0, paste | xclip owns the clipboard on the same Xvfb display; Ctrl+V; the text line must show typed plus pasted text | no clipboard owner |
| 4a | `--probe` reports "Add to Chrome" present and enabled. OSR: a real click at the button's centre (the probe confirms a trusted click). Chrome and Flecha: Return on the focused button, then Tab and Return in the install dialog (keyboard only). Pass: Dark Reader is unpacked in the profile | the same page, nothing pressed |
| 4b | `https://example.com/` loads (probe), its computed `html` or `body` background is dark, the Chrome-style window title is the page's, and an Xvfb screenshot of the page area has a mean grey below 128 | the same page on the control profile, which holds no Dark Reader |
| 5a | the demo login is typed and submitted. Chrome and Flecha: Alt+Shift+A focuses the save bubble, Return saves. OSR has no save prompt or callback. Pass: the profile's `Login Data` (read with `sqlite3`, read-only) holds a login for the demo site | the same run without pressing Save |
| 5b | on a profile holding the saved login, a real xdotool click on the page; the probe must see both fields non-empty | a profile with nothing saved |
| 6 | the spike runs with `--fake-camera`; the page's camera button calls `getUserMedia`. OSR: the click prints a `[permission N]` prompt and `allow N` answers it. Chrome and Flecha: Tab and Space press the button, Tab and Return accept the bubble. Pass: one granted video track | the same script without `--fake-camera`: no device |
| 7 | OSR: Ctrl+P prints `[print] start`, then `pdf` writes a PDF whose text (`pdftotext`) holds the page's phrase. Chrome and Flecha: Ctrl+P, Return on the preview's Save, the path typed into the Save File dialog; pass when that PDF holds the phrase | no Ctrl+P, on a page without the phrase |
| `--fake-camera` guard | see below | the same script without `--fake-camera` must reach the fake remote origin six times |

#### `--fake-camera` and `--probe`

Xvfb has no camera, so check 6 needs Chromium's `--use-fake-device-for-media-stream`, which is refused after `--` like every other forbidden switch. The spike adds it only through its own `--fake-camera` option, and the binary then loads only `file://` and loopback (`127.0.0.1`, `localhost`, `[::1]`) URLs: the start URL (exit 2 otherwise), `go`, navigations, redirects, popups and every subresource (`LocalOnlyRequestHandler` in `src/common.rs`). `--fake-camera` together with `--self-check` or `--probe` is refused too. So the fake device cannot be combined with a sign-in page. `scripts/fake-camera-guard.sh` tests this in both modes against a second fixture server on `127.0.0.2`, standing in for a remote origin: a frame, an image, a fetch, a redirected fetch, a navigation and a redirected navigation must all be blocked, and in OSR `go https://accounts.google.com/` and `go https://mail.google.com/` must be refused before any request. The control run without the flag must reach `127.0.0.2` with all six. Unit tests in `src/cli.rs` and `src/common.rs` cover the CLI refusals and the URL rule; `auto-checks.sh` also checks the refusals under `env -i` with no display before any launch.

Today's Flecha has no such rule in its binary. `scripts/flecha-launch.sh` refuses a non-local start URL with `--fake-camera`, but pages can still navigate on their own. The Flecha runs open only `scripts/input-test.html` on a scratch profile with no account.

`--probe` injects a fixed script into the checklist's fixed pages (consent, the Dark Reader store page, example.com, the herokuapp login and secure pages) and prints `[probe] {json}` reports. A report carries only booleans, numbers and short strings from a small alphabet, behind a per-run nonce (`src/probe.rs`, with a test), so it can never print page text or field values. The only input it gives is focus, and "Reject all" on Google's consent page.

#### Findings

- **The OSR Web Store install crashes the browser process.** Clicking "Add to Chrome" in OSR crashed the app with SIGSEGV (`app exit 139`) in both recorded runs (`results/auto-checks/run1-evidence/store-osr.log`, `run2-evidence/store-osr.log`) and in the PR verifier's own run. Logs of the earlier attempts were not kept. The host's systemd-coredump store holds 14 SIGSEGV cores from this branch's binary, all with the extension install dialog on the stack; 3 of them crash one frame later, in `views::BubbleFrameView::SetTitleView` called from `ExtensionInstallDialogView::AddedToWidget`. An earlier count of "11 of 13 attempts" could not be traced to logs and is withdrawn. At least one earlier attempt did not crash but installed nothing within 30 seconds. No attempt installed Dark Reader. The runner marks 4b `not reached` when 4a installed nothing without a crash; on the final code that branch never ran, since every final run crashed. The stack (from systemd-coredump): `ui::PropertyHandler::GetPropertyInternal` ← `ChromeConstrainedWindowViewsClient::GetModalDialogHost` ← `AlloyConstrainedWindowViewsClient::GetModalDialogHost` ← `constrained_window::CreateBrowserModalDialogViews` ← `ShowExtensionInstallDialogImpl` ← `ExtensionInstallPrompt::ShowConfirmation` ← `WebstorePrivateBeginInstallWithManifest3Function::ShowInstallDialog`. My reading of the stack, not checked against Chromium's source: Alloy has no browser window to host the browser-modal install dialog, and the dialog-host lookup dereferences that missing window. The pinned CEF 158 headers have no extension-handler API, so a GPUI shell would need its own install confirmation, or another way to install extensions.
- **The Dark Reader content script in OSR is unreliable.** On a copy of the Chrome-style profile with Dark Reader installed, example.com turned dark in 1 of 5 OSR runs before these recorded runs; in the other 4 it stayed light, and a reload did not help. `chrome://extensions` is refused in OSR. In all 9 full runner runs (the `content-osr` case) it stayed light: 1 dark result in 14 OSR runs. The Chrome-style control turned example.com dark in all 9.
- **Password fill works in OSR; save is not offered.** On a copy of the profile where Chrome mode saved the demo login, OSR filled both fields after a real click. OSR shows no save prompt and gives no callback, and the OSR save run left 0 logins in `Login Data`.
- **Paste from the X clipboard fails in OSR on Xvfb.** With xclip owning the clipboard on the same display, Ctrl+V inserted nothing, and xclip never received a selection request, so Chromium never asked the X server for the clipboard. Chrome mode pasted the same text. Check 0 on the real desktop is the first paste test that counts.
- **Input pass rate (issue #94).** On this branch's final binary and page, `input-actions.sh` passed 12 of 12 runs ([summary](results/auto-checks/input-runs.txt); 6 of those before the last `input-test.html` change). In an earlier batch, before the last self-check rebuild, it passed 2 of 3. The failing run lost one keystroke in the password field: the page counted 8 of the 9 characters of `pa$$ wörd`, while the click and the focus wait worked. The password field does not log key presses, so which key was lost is unknown. One untested hypothesis is the `ö`: xdotool types it by remapping a spare keycode for the length of the key press. So the rate is not 100 %: #94 measured 8 of 10, and this branch 14 of 15 in total. `check0-input.sh` types on a real Swedish keymap, where `åäö` and AltGr characters have their own keys, so no remap is involved: its typing step passed in all 9 full runs in each of the three columns (27 of 27). Paste passed 9 of 9 in Chrome-style and Flecha and 0 of 9 in OSR.
- **Chromium starts its own unsandboxed zygote with `--no-zygote-sandbox`.** Chromium's Linux zygote layout includes one zygote launched as `--type=zygote --no-zygote-sandbox` (for processes that run without the sandbox, such as the GPU process). With `no-zygote-sandbox` in the forbidden list, the self-check flagged that process. The self-check now accepts `--no-zygote-sandbox` only on a `--type=zygote` process (`forbidden_in_process` in `src/selfcheck.rs`, with a test); the CLI still refuses it after `--`.
- **A control caught a harness race.** In one full run before the recorded ones, the OSR guard's control failed: the unguarded top-level navigation to `127.0.0.2/navigate` ended with `ERR_ABORTED`, because the script sent the next `go` before the first navigation had settled. The runner exited 1, as it should. The script now starts the second navigation only after the first one settled; after the fix the guard and its control passed 3 of 3 times in both modes. In a later pair of full runs, Flecha's camera step failed once: of the eight Tabs meant to reach the camera button, one never reached the page, so Space pressed the notification button. The step now presses Tab until the page reports focus on the camera button; after that, check 6 and its control behaved as expected 3 of 3 times in all three modes.
- **URLs typed with `go` no longer reach CEF's warnings (issue #94).** CEF logged a URL it could not parse in full (`Invalid URL: …`), and a `filesystem:` URL made Chromium's storage code log the whole URI to `chrome_debug.log`. `go`, and `--url` in OSR mode, now go through `check_typed_url` in `src/common.rs` first. In Chrome mode, `--url` reaches `chrome::create_browser` from `on_context_initialized` before that check runs; `cli::parse` has already refused any non-local `--url` by then. With the check, only `http`, `https`, `file`, `about`, `chrome` and `data` URLs that CEF parses are passed on, and a refusal names only the scheme or origin.
- **Flecha reference.** Flecha 158.0.8127.2 (`~/.local/opt/flecha-current`, only read) runs isolated through `scripts/flecha-launch.sh` (never the `flecha` launcher, a scratch profile, a copy of the install's `shell/`). Checks 0, 6 and 7 are automated. 4a, 4b, 5a and 5b are not: Flecha has no `--probe`, its first-run tour covers the page, and a copied profile did not carry Dark Reader over. They stay manual.

#### Isolation disclosures

- Before this branch, Chromium wrote its `org.chromium.Chromium.*` singleton directories into the host's `/tmp` during harness runs, because the harness did not set `TMPDIR`. `xvfb-isolated.sh` now gives every run its own TMPDIR. There are 183 such directories in `/tmp` from earlier runs that cannot be attributed to one run or agent; they were left in place.
- Each OSR store crash leaves a core dump in the host's systemd-coredump store, outside the run's scratch directory; `coredumpctl` lists 14 from this branch's runs. `ulimit -c 0` in `xvfb-isolated.sh` would stop this; it is not in this branch. The stack above came from `coredumpctl info` for that crashed process; `journalctl --user` was read for the same crash. No privileged or system-bus command, no `sudo`, and no `cargo` inside `target/spike1-src/zed` was run.

### Isolation note

The first OSR self-check ran under a plain `dbus-run-session`. D-Bus activation on that private bus started a keyring helper that printed `GNOME_KEYRING_CONTROL=/run/user/1000/keyring`, the owner's keyring control directory, and started portal and gvfs services. So that run may have reached the owner's running `gnome-keyring-daemon` through its control socket. The run's logs show no secret request and no unlock prompt, but logs alone cannot rule out that the helper talked to the daemon. Every later run used `scripts/session-bus.conf` (no service directories, so nothing is activated) and a scratch `XDG_RUNTIME_DIR`, and printed no keyring, portal or gvfs lines.

Until the third review round the harness left `DBUS_SYSTEM_BUS_ADDRESS` unset, so libdbus and GDBus could fall back to the host's system bus socket. The results in this report come from runs with it pointed at a path that does not exist.

## Manual checklist for the owner

`scripts/auto-checks.sh` now runs every checklist step that needs no person (see [Checklist rows on Xvfb](#checklist-rows-on-xvfb)). What is left needs you, your real display and a Google account made for this test:

- checks 1 to 3, Google sign-in, in both modes and in today's Flecha;
- check 0 on your real keyboard, input method and display (OSR only);
- check 4c in Chrome mode, the extension popup, which opens from Chrome's own toolbar;
- checks 4a, 4b, 5a and 5b in today's Flecha, which the runner cannot drive (no probe, and its first-run tour covers the page).

Rules for the whole run:

- Enter the account's password and MFA code yourself. The app logs no keystrokes or form contents. The terminal shows load events, popups and permission requests with each URL reduced to its origin (see [Console output and logs](#console-output-and-logs)).
- **Never type a real password or MFA code into `input-test.html`** (check 0). Its console output reaches the terminal.
- **Take no screenshots that show the account's name, address or picture**, and do not paste account identifiers into this report.
- **Do not use `--log-console`** during checks 1 to 3.
- **Decide on the password store first** (see [Keyring or basic password store](#keyring-or-basic-password-store-an-owner-choice-for-checks-1-and-5)) and note it below. The automated rows always used `--password-store=basic`.

From `rewrite/`, after [Build and run](#build-and-run):

```sh
s=spikes/cef-osr-signin
# Check 0 (OSR, made-up text only)
env -u WAYLAND_DISPLAY $s/run.sh --mode osr --profile "$HOME/flecha-spike0/input" --url "file://$PWD/$s/scripts/input-test.html"
# Checks 1 to 3, one fresh profile per mode; add -- --password-store=basic if you chose it
env -u WAYLAND_DISPLAY $s/run.sh --mode osr --profile "$HOME/flecha-spike0/osr" --url https://accounts.google.com/
env -u WAYLAND_DISPLAY $s/run.sh --mode chrome --profile "$HOME/flecha-spike0/chrome" --url https://accounts.google.com/
# Check 4c (Chrome mode, its own profile)
env -u WAYLAND_DISPLAY $s/run.sh --mode chrome --profile "$HOME/flecha-spike0/store" --url https://chromewebstore.google.com/detail/dark-reader/eimadpbcbfnmbkopoojfekhnkhdbieeh
```

Delete `$HOME/flecha-spike0` afterwards.

0. **Typing on your real display (OSR only).** In the first text field of `input-test.html` type `@`, `åäö`, a few AltGr characters (`{[]}\`, `€`) and a Shift capital. Click into the field right after the page appears, and again after switching back from the terminal; note whether a click was lost. Paste text copied from another app with Ctrl+V. Pass: the terminal's `[console] INPUT text "…"` lines show exactly what you typed and pasted. On Xvfb paste failed (see [Checklist rows on Xvfb](#checklist-rows-on-xvfb)), so this is the first paste test on a real desktop. If any character is wrong or missing, stop: the sign-in checks would fail for input reasons.
1. **Gmail sign-in with MFA, then restart.** Sign in, complete MFA, open `https://mail.google.com/`. Close the window (OSR: or type `quit`) and wait for the exit. Start again with the same `--profile` and `--url https://mail.google.com/`. Pass: the inbox loads without a sign-in prompt. If Google shows "This browser or app may not be secure" or similar, record the wording. **If this fails in OSR mode, stop and return to the plan owner** (plan rule).
2. **Sign out and back in.** In Gmail, sign out from the account menu, then sign in again with MFA. Pass: the inbox loads.
3. **"Sign in with Google" on a third-party site** you already trust. Note whether it opened a popup or redirected. Pass: you end up signed in there. OSR: the popup opens as a second window; it must close itself and the first window must be signed in.
4. **4c, Chrome mode only.** Click "Add to Chrome" and accept the dialog (or reuse the profile after 4a), then open the puzzle-piece icon in the toolbar and choose Dark Reader. Pass: the popup opens.
5. **Flecha reference, 4a, 4b, 5a, 5b.** In today's Flecha with a new profile: install Dark Reader from the store link above and open `https://example.com/` (pass: the page is dark); log in at `https://the-internet.herokuapp.com/login` with the public demo login `tomsmith` / `SuperSecretPassword!` (pass: Flecha offers to save it), log out, reload (pass: the fields fill or a click offers the login).

### Results (fill in; pass, fail or n/a, plus a short note)

Password store used for checks 1 to 3 (default keyring or `--password-store=basic`): ______

| # | Check | Alloy OSR | Chrome-style control | Flecha (reference) |
|---|---|---|---|---|
| 0 | Typing and paste on the real display | | n/a | n/a |
| 1 | Gmail sign-in with MFA, survives restart | | | |
| 2 | Sign out and back in | | | |
| 3 | Third-party "Sign in with Google" | | | |
| 4a | Web Store: install completes | automated | automated | |
| 4b | Extension content script runs | automated | automated | |
| 4c | Extension popup opens | automated (no surface) | | |
| 5a | Password save offered | automated | automated | |
| 5b | Saved password fills | automated | automated | |
| 6 | Camera permission through a host prompt | automated | automated | automated |
| 7 | Print | automated | automated | automated |

"automated" cells are in [Checklist rows on Xvfb](#checklist-rows-on-xvfb).

## What I could not verify

- **Google sign-in.** Checks 1 to 3 need a person and an account. None were attempted. No account was signed in to.
- **A real camera.** Check 6 ran with Chromium's fake capture device (`--fake-camera`), so the prompt path ran in both modes, but no real camera driver or device picker was involved.
- **The Chrome-mode extension popup (4c)** and **Flecha's 4a, 4b, 5a and 5b.** They need Chrome's own toolbar, or a page probe Flecha does not have. Manual.
- **Paste and the owner's real keyboard.** AltGr, `åäö` and paste were tested with xdotool and xclip on an Xvfb Swedish layout. Paste failed in OSR there. Check 0 on the real desktop covers both.
- **Native Wayland.** Every run used Xvfb (X11). The native-Wayland command in [Build and run](#build-and-run) was checked only to the point of CEF selecting its Wayland platform; no compositor was run.
- **HiDPI.** Xvfb runs at scale 1. The OSR code passes the window scale to CEF (`GetScreenInfo`) and converts mouse coordinates, but no scale other than 1 was tested.
- **OSR features this spike does not implement:** IME composition, context menus, drag and drop, JavaScript `alert`/`confirm`/`beforeunload` dialogs, file pickers, clipboard to other apps, DevTools, an address bar. If a sign-in page depends on one of these, the failure belongs to this spike, not to Alloy. Note it.
- **A lost first click in OSR.** Sometimes a click sent right after the page's first `load-end` reaches nothing. It happened in one early scripted run. The second independent verifier saw it in 1 of 4 input runs: the click on the text field was lost and the typed keys went to the document. The first verifier did not see it in 3 of 3. The cause is unknown; it may be harness timing (the click arriving before the window is ready for input) or a real OSR focus problem. The input script now waits for the field's `focus` event before typing, so a lost click fails the run instead of typing into the wrong place. With that wait, 6 of 6 input runs passed in the fresh clone and 14 of 15 on this branch; the one failure lost a keystroke, not the click (see [Checklist rows on Xvfb](#checklist-rows-on-xvfb)). Check 0 asks the owner to watch for it.
- **The owner's login keyring.** Every automated run used a private D-Bus bus with no keyring, so the browser fell back to the basic store. The keyring path on the owner's session is untested; see the password-store choice in [Build and run](#build-and-run).
- **Caps Lock state in OSR.** winit 0.30 does not report lock-key state, so the Caps Lock flag is never sent to CEF. Typed text is unaffected (it comes from the layout), but a page reading `getModifierState('CapsLock')`, as some password fields do for a warning, sees it off.
- **Keys with no US physical-key mapping in OSR.** Text from them is inserted correctly through `CHAR` events, but the page's `keydown` sees `key` as a NUL character and `keyCode` 0 (seen with xdotool's remapped `ö`, [log](evidence/osr-input-events.txt)). A real Swedish layout maps `ö` to a physical key, so this is expected to matter only for unusual layouts.
- **Punctuation keys on remapped layouts in OSR.** A letter key's Windows key code comes from the layout; any other key's comes from its physical position. On US Dvorak the physical Q key types an apostrophe but still reports `VK_Q`, so a page that reads `keyCode` sees Ctrl+apostrophe as Ctrl+Q. Typed text is unaffected.
- **A stray key-up after Ctrl+P in OSR.** The app consumes the Ctrl+P key press but forwards its release, so the page sees a `keyup` for P with no `keydown`. Pages ignore unmatched key-ups; it was not fixed.

### Known limits of the runtime checks and harness

These were reviewed and left as they are. Each either fails closed (stops with an error rather than running the wrong thing) or does not touch the sign-in path.

- An inline comment after a value in `rewrite/pins.toml` (`version = "…" # note`) is accepted by `prepare.sh` but makes `build.rs`'s pin parser panic, so the build stops. Keep pins without inline comments.
- The staged runtime directory is keyed by CEF's numeric version, not by archive. Switching between two archives of the same version (for example beta and stable) leaves the old stage in place; `run.sh`, `build.sh` and the binary then refuse it because `archive.json` names the other archive. Remove `target/cef-runtime` and run `prepare.sh` again.
- `archive.json` records which archive was staged; the binary checks it, the commit and the API version, but not a checksum of every staged file. Replacing `libcef.so` with another build of the same CEF commit goes undetected.
- `runtime.rs` finds the loaded `libcef.so` in `/proc/self/maps` by splitting on spaces. A checkout path containing a space makes the check fail and the app exit; it never accepts a wrong library.
- The self-check injects its collector into every main-frame page of the current step and binds results to the first browser and a per-run nonce, not to one navigation. A page reached by an unexpected redirect could report for the step. The steps load fixed internal pages and the Web Store; the low risk was accepted.
- In chrome mode a failed first browser creation quits the message loop through a posted task. If CEF also refused that task, the app would hang instead of exiting. That needs CEF to refuse two calls in a row; not handled.
- `xvfb-isolated.sh` signals the app's process group only while the app's wrapper (`dbus-run-session`) is still running. If the wrapper exited after a browser crash, browser children left in that group would not be stopped. No leftover processes were seen in any run.
- Without `--mode`, an argument that looks like `--type` makes `main` hand the process to CEF's subprocess entry point before the CLI parser runs, even after `--` and in its single-dash or valueless forms (`-- -type=bogus`). The process then runs CEF's `execute_process` with those arguments, and the CLI parser and its error message are skipped. Always pass `--mode`.
- `run.sh` resolves a bare `FLECHA_SPIKE_BIN` with `command -v`, which finds shell builtins first: `FLECHA_SPIKE_BIN=printf` is refused even with an executable of that name on `PATH`. Pass a path that contains `/`.
- `prepare.sh` checks the staged payload with `-e`, so a directory named `resources.pak` passes it. The binary's start-up check then refuses the runtime.
- The console checks count after the marker page's `/ran` request and a round trip through another local page, not after an acknowledgement for each console callback. A marker that CEF delivered later than that would be missed: a `--log-console` run would fail, a default run would pass.
- Console relay checks the frames that exist when CEF reports the message. A remote iframe removed between logging and that report is no longer counted.
- `input-actions.sh` asserts `[print] start` and that the app is still running a moment later. It does not wait for Chromium's no-printer error, so a crash after that point would go unnoticed by the script.
