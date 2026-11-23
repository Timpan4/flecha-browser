# Spike 0: Alloy OSR sign-in and Chrome features

Plan: [docs/cef-gpui-rewrite-plan.md](../../../docs/cef-gpui-rewrite-plan.md), section "Spike 0".

## Status

- Built on the pins and self-checked in both modes on Xvfb. Results are in [Automated results](#automated-results).
- The sign-in gate (checks 1 to 7) is **not run**. It needs the owner and a purpose-made Google account. The checklist is in [Manual checklist](#manual-checklist-for-the-owner).
- The automated checks found one result that matters before the manual run: on the Chrome Web Store, "Add to Chrome" is **disabled in both modes**, even in the Chrome-style control (see [Web Store](#web-store)).

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
| cef-rs | `Timpan4/cef-rs` rev `32d6bc55` | Built from that rev |
| CEF version | `158.0.3+g8c29be2+chromium-158.0.8127.18` | `chrome://version`: `CEF 158.0.3+g8c29be2+chromium-158.0.8127.18`, `Chromium 158.0.8127.18 (Official Build)` |
| CEF commit | `g8c29be2` | `cef_api_hash(…, 2)` at run time: `8c29be240971eebee960ac92114d3efccb5780ff` |
| Archive | beta SHA-1 `057a0b87…`, stable SHA-1 `620b80a0…` | `cef_binary_158.0.3+g8c29be2+chromium-158.0.8127.18_linux64_minimal.tar.bz2`, SHA-1 `620b80a02dfe93ed7884a9eb1b265ff34f799024`: the **stable** archive of the same commit. This is the known gap in the plan's "Beta channel rule" (cef-rs picks the first index entry). |

## Build and run

Run from `rewrite/`. Builds go through the shared lock (see [rewrite/README.md](../../README.md)):

```sh
flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
  bash ~/.claude/skills/hosts/scripts/agent-build run cargo build -j 14 -p cef-osr-signin
```

The first build downloads the CEF archive (about 1.5 GB unpacked) and checks its SHA-1 against the CEF index.

Run with a new, dedicated profile directory. Never point `--profile` at a real browser profile or at `.flecha-profile`.

```sh
# Alloy OSR
./target/debug/cef-osr-signin --mode osr --profile "$HOME/flecha-spike0/osr" --url https://accounts.google.com/

# Chrome-style control
./target/debug/cef-osr-signin --mode chrome --profile "$HOME/flecha-spike0/chrome" --url https://accounts.google.com/
```

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
./target/debug/cef-osr-signin --mode osr --profile /tmp/spike0-check/osr \
  --self-check --self-check-timeout-secs 30 --report /tmp/spike0-check/osr.json
```

`--self-check-timeout-secs` is required. It bounds each step; 30 is the value used for the runs below, not a product limit.

To run anything on Xvfb with an isolated session (no D-Bus activation, scratch HOME, XDG and runtime dirs), use `spikes/cef-osr-signin/scripts/xvfb-isolated.sh <out-dir> <app args…>`.

### Workarounds in this spike

- `rewrite/.cargo/config.toml` sets `net.git-fetch-with-cli = true`. Cargo's built-in git client failed to fetch the pinned commit on the workstation (`revision … not found`, through the user's `url.insteadOf` SSH rewrite). The git CLI fetches it.
- `build.rs` copies the CEF runtime next to the binary. At the pinned rev, cef-dll-sys copies it to `OUT_DIR`'s third parent, which under Cargo 1.100's build-dir layout is `target/debug/build`, not `target/debug` ([tauri-apps/cef-rs#522](https://github.com/tauri-apps/cef-rs/pull/522), open). Without this, `libcef.so` and the `.pak` files are missing at run time. `cef-dll-sys` is a direct dependency only so `build.rs` receives `DEP_CEF_DLL_WRAPPER_CEF_DIR`. Remove both once the pin includes #522.
- `build.rs` sets RUNPATH `$ORIGIN`, so no `LD_LIBRARY_PATH` is needed.

## Automated results

RESULTS_PLACEHOLDER

## Manual checklist for the owner

Use a Google account made for this test. Enter its password and MFA code yourself. Nothing in the app logs keystrokes or form contents; the terminal shows only page `console.log` output, load events and permission requests. Do not paste account identifiers into this report.

Use a fresh `--profile` per mode, for example `$HOME/flecha-spike0/osr` and `$HOME/flecha-spike0/chrome`. Run every check in both modes. Run the same checks in today's Flecha as the reference column.

1. **Gmail sign-in with MFA, then restart.**
   1. Start the mode with `--url https://accounts.google.com/`.
   2. Sign in, complete MFA, open `https://mail.google.com/` and confirm the inbox loads.
   3. Close the window (OSR: or type `quit`). Wait for the process to exit.
   4. Start again with the same `--profile` and `--url https://mail.google.com/`.
   5. Pass: the inbox loads without a sign-in prompt. If Google shows "This browser or app may not be secure" or similar, record the exact wording.
   6. **If this fails in OSR mode, stop and return to the plan owner** (plan rule).
2. **Sign out and back in.** In Gmail, sign out from the account menu, then sign in again with MFA. Pass: the inbox loads.
3. **"Sign in with Google" on a third-party site.** Use a site you already trust that offers "Sign in with Google". Note whether it opened a popup window or redirected. Pass: you end up signed in on that site. OSR: a popup opens as a second window; it must close itself and the first window must be signed in.
4. **Web Store install, content script and popup.** Open `https://chromewebstore.google.com/detail/dark-reader/eimadpbcbfnmbkopoojfekhnkhdbieeh` (Dark Reader: a content script that recolours pages, and a toolbar popup).
   1. If a consent page appears, choose "Reject all".
   2. Record whether "Add to Chrome" is enabled. Click it and accept the install dialog.
   3. Open `https://example.com/`. Pass (content script): the page turns dark.
   4. Pass (popup): open the extension's popup. Chrome mode: puzzle-piece icon in the toolbar, then Dark Reader. OSR mode has no toolbar, so record "no surface to open a popup" unless something appears.
5. **Save a password and fill it back.** This needs no real credentials: `https://the-internet.herokuapp.com/login` is a public demo with user `tomsmith` and password `SuperSecretPassword!`.
   1. Log in there. Pass (save): the browser offers to save the password, and you save it.
   2. Log out, reload the login page. Pass (fill): the fields fill in, or a click in the username field offers the saved login.
6. **Camera permission through a host prompt.** Open `https://permission.site/` and click "Camera".
   - Chrome mode: Chrome's permission bubble appears. Allow it.
   - OSR mode: the terminal prints `[permission N] https://permission.site requests media access (camera; …)`. Type `allow N`.
   - Pass: the site reports camera access granted. The machine needs a camera; with none, record "no camera".
7. **Print.** On any page press Ctrl+P.
   - Chrome mode: pass if the print preview opens.
   - OSR mode: record what Ctrl+P does (on Xvfb it did nothing visible). Then type `pdf` and check that `<profile>/flecha-spike-print.pdf` contains the page. The plan expects "Use `Print` or a GPUI preview over `PrintToPDF`".

### Results (fill in; pass, fail or n/a, plus a short note)

| # | Check | Alloy OSR | Chrome-style control | Flecha (reference) |
|---|---|---|---|---|
| 1 | Gmail sign-in with MFA, survives restart | | | |
| 2 | Sign out and back in | | | |
| 3 | Third-party "Sign in with Google" | | | |
| 4a | Web Store: "Add to Chrome" enabled and install completes | | | |
| 4b | Extension content script runs | | | |
| 4c | Extension popup opens | | | |
| 5a | Password save offered | | | |
| 5b | Saved password fills | | | |
| 6 | Camera permission through a host prompt | | | |
| 7 | Print | | | |

## What I could not verify

- **Google sign-in, Web Store installs, password save and fill, camera, print.** All need a person, an account or hardware. None were attempted. No account was signed in to.
- **Native Wayland.** Every run used Xvfb (X11). On the owner's Wayland session winit picks Wayland for the OSR window, and CEF picks its own Ozone platform. Neither was run there.
- **HiDPI.** Xvfb runs at scale 1. The OSR code passes the window scale to CEF (`GetScreenInfo`) and converts mouse coordinates, but no scale other than 1 was tested.
- **OSR features this spike does not implement:** IME composition, context menus, drag and drop, JavaScript `alert`/`confirm`/`beforeunload` dialogs, file pickers, clipboard to other apps, DevTools, an address bar. If a sign-in page depends on one of these, the failure belongs to this spike, not to Alloy. Note it.
- **Keys with no US physical-key mapping in OSR.** Text from them is inserted correctly through `CHAR` events, but the page's `keydown` sees an empty `key` and `keyCode` 0 (seen with xdotool's remapped `ö`). A real Swedish layout maps `ö` to a physical key, so this is expected to matter only for unusual layouts.
- **Beta archive.** The build uses the stable archive of the pinned commit, not the beta archive (pins.toml SHA-1 `057a0b87…`).
