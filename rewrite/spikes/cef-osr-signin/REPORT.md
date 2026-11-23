# Spike 0: Alloy OSR sign-in and Chrome features

Plan: [docs/cef-gpui-rewrite-plan.md](../../../docs/cef-gpui-rewrite-plan.md), section "Spike 0".

## Status

- Built on the pins and self-checked in both modes on Xvfb. On Xvfb, OSR input, the `window.open` popup, a notification permission prompt, printing to PDF, the terminal commands, restart persistence, and Chrome-mode tab handling also passed. Results are in [Automated results](#automated-results).
- The sign-in gate (checks 1 to 7) is **not run**. It needs the owner and a purpose-made Google account. The checklist is in [Manual checklist](#manual-checklist-for-the-owner).
- Nothing in the automated results blocks the manual run. Both modes present the same identity to pages: `navigator.webdriver` false, CEF's default user agent, `Chromium` and `Not.A/Brand` brands with no `Google Chrome` brand. Both run fully sandboxed. The Web Store offers "Add to Chrome" in both modes, next to an "Install Chrome" button (see [Web Store](#web-store)).

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
env -u WAYLAND_DISPLAY ./target/debug/cef-osr-signin --mode osr \
  --profile "$HOME/flecha-spike0/osr" --url https://accounts.google.com/

# Chrome-style control
env -u WAYLAND_DISPLAY ./target/debug/cef-osr-signin --mode chrome \
  --profile "$HOME/flecha-spike0/chrome" --url https://accounts.google.com/
```

`env -u WAYLAND_DISPLAY` makes winit and CEF both use X11 (XWayland on a Wayland desktop). That is the only path tested: every automated run was on Xvfb. Launching with `WAYLAND_DISPLAY` set has not been run.

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

To run anything on Xvfb with an isolated session (no D-Bus activation, scratch HOME, XDG and runtime dirs), use `spikes/cef-osr-signin/scripts/xvfb-isolated.sh <out-dir> <app args…>`. The app's stdin is the FIFO `<out-dir>/stdin.fifo`, so OSR commands can be sent to it. With `ACTIONS=<script>` the harness waits for the first page load, runs the script (with `DISPLAY`, `OUT`, `APP_PID` and `CMD` set), and returns the app's exit status if the script ended the app. The action scripts in `scripts/` need:

- `XDOTOOL`: path to xdotool. It is not installed on the workstation; the runs used nixpkgs' build (`nix --extra-experimental-features 'nix-command flakes' build nixpkgs#xdotool`).
- `WAIT_SECS`: how long each step waits for its log line. The runs used 30. It bounds a test run and is not a product limit.
- `WM` (only `persist-actions.sh`): an EWMH window manager binary. Xvfb has none, and closing a window the way a desktop does needs one. The runs used nixpkgs' openbox 3.6.1.

Example, from `rewrite/`:

```sh
ACTIONS=$PWD/spikes/cef-osr-signin/scripts/osr-actions.sh WAIT_SECS=30 XDOTOOL=/nix/store/…-xdotool-…/bin/xdotool \
  bash spikes/cef-osr-signin/scripts/xvfb-isolated.sh /tmp/spike0-osr --mode osr --profile /tmp/spike0-osr/profile \
  --url "file://$PWD/spikes/cef-osr-signin/scripts/input-test.html"
```

### Workarounds in this spike

- `rewrite/.cargo/config.toml` sets `net.git-fetch-with-cli = true`. Cargo's built-in git client failed to fetch the pinned commit on the workstation (`revision … not found`, through the user's `url.insteadOf` SSH rewrite). The git CLI fetches it.
- `build.rs` copies the CEF runtime next to the binary. At the pinned rev, cef-dll-sys copies it to `OUT_DIR`'s third parent, which under Cargo 1.100's build-dir layout is `target/debug/build`, not `target/debug` ([tauri-apps/cef-rs#522](https://github.com/tauri-apps/cef-rs/pull/522), open). Without this, `libcef.so` and the `.pak` files are missing at run time. `cef-dll-sys` is a direct dependency only so `build.rs` receives `DEP_CEF_DLL_WRAPPER_CEF_DIR`. Remove both once the pin includes #522.
- `build.rs` sets RUNPATH `$ORIGIN`, so no `LD_LIBRARY_PATH` is needed.

## Automated results

Run on 2026-11-23 on the workstation, on Xvfb only, through `scripts/xvfb-isolated.sh` (private D-Bus bus without service activation, scratch HOME, XDG and runtime dirs, fresh profile per run). Each run is one `--self-check` with `--self-check-timeout-secs 30`. The JSON reports are in [evidence/](evidence/).

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
| /proc: renderer-type processes | seccomp mode 2, `NoNewPrivs` 1, own user, PID and network namespaces | same |
| /proc: processes without seccomp | browser, network service (`--utility-sub-type=network.mojom.NetworkService`), zygote helpers; Chromium's normal Linux layout | same |
| Forbidden switches in any process command line (`--remote-debugging-*`, `--enable-automation`, `--headless`, `--no-sandbox` and other sandbox-off switches, `--user-agent`, `--disable-web-security`, `--ignore-certificate-errors`, `--use-mock-keychain`, `--test-type`) | none | none |

Switches CEF itself adds to the browser process (from `chrome://version`, identical in both modes): `--lang=en-US --disable-signin-promo-on-avatar-pill-for-testing --disable-features=GlicActorUi,AutofillActorMode,LensOverlay,KillOnInvalidNavigationHeaders --ozone-platform=x11`. None of them is visible to pages. Judging only by its name, `--disable-signin-promo-on-avatar-pill-for-testing` hides a sign-in promo on the profile avatar button; I did not check Chromium's source for what else it does.

### Web Store

Page: uBlock Origin Lite, `https://chromewebstore.google.com/detail/ublock-origin-lite/ddkjiahejlhfcafbddmgiahcphecmpfh`. Nothing was clicked except "Reject all" on Google's consent page, which the store redirects to from a Swedish address (`consent.google.com/…&gl=SE`). Nothing was installed and no account was used.

| Field | Alloy OSR | Chrome-style control |
|---|---|---|
| Final page | the detail page, after the consent redirect | same |
| "Add to Chrome" | present, enabled | present, enabled |
| Other offer | "Install Chrome" button, enabled | same |
| `typeof chrome.webstorePrivate` | `object` | `object` |
| `typeof chrome.management` | `object` | `object` |

"Add to Chrome" first renders disabled and turns enabled once the page has finished its own checks. An earlier run that reported at first render saw it disabled; the self-check now waits for the enabled state. The "Install Chrome" button suggests the store does not take either mode for Google Chrome (no `Google Chrome` brand in `userAgentData`). Whether clicking "Add to Chrome" installs anything is check 4, for the owner. In OSR there is no extension toolbar, so even a successful install has no surface for the popup.

### OSR input on Xvfb

`scripts/input-actions.sh` drives `scripts/input-test.html` in an OSR window with xdotool. xdotool is not installed on the workstation; the run used nixpkgs' build (`nix --extra-experimental-features 'nix-command flakes' shell nixpkgs#xdotool`, passed as `XDOTOOL=<path>`, run without the inherited `LD_LIBRARY_PATH`). The page logs what it receives ([full log](evidence/osr-input-events.txt)):

| Action | Page saw |
|---|---|
| Click a text field, type `Hello Wörld 123!` | value `Hello Wörld 123!` |
| Click a password field, type `pa$$ wörd`, Enter | value `pa$$ wörd`, `keydown Enter` |
| Textarea: `line1`, Enter, `line2`, Ctrl+A, Backspace, `kept` | `line1\nline2`, then empty, then `kept` |
| Double click a button | `click` detail 1, `click` detail 2, `dblclick` |
| Click a `<select>`, Down, Enter | popup drawn over the page ([screenshot](evidence/osr-select-popup.png)), `change` to `two` |
| Three wheel notches down | `scroll` events up to `scrollY` 240 |
| Ctrl+P | no output, no crash, no visible dialog |

Screenshots: [OSR page](evidence/osr-page.png), [Chrome-style control](evidence/chrome-page.png).

### Behaviour checks on Xvfb

Each row is one scripted run through `scripts/xvfb-isolated.sh` on `scripts/input-test.html`, with `WAIT_SECS=30`. Logs: [OSR](evidence/osr-actions.txt), [Chrome](evidence/chrome-actions.txt), [OSR restart](evidence/osr-persist.txt), [Chrome restart](evidence/chrome-persist.txt).

| Check | Script | Result |
|---|---|---|
| OSR `window.open` popup | `osr-actions.sh` | A second OSR window opened and painted ([screenshot](evidence/osr-popup.png)); the popup page saw `window.opener` non-null; `window.close()` in the popup closed only the popup (open browsers 2, then 1) and the app kept running |
| OSR notification permission | `osr-actions.sh` | `Notification.requestPermission()` printed `[permission 1] file:/// requests permission types 0x8000`; `allow 1` on stdin resolved it as `granted` |
| OSR camera permission | `osr-actions.sh` | `getUserMedia({video: true})` failed with `NotFoundError` and **no prompt**: Xvfb has no camera, and the request ends before any permission callback. The camera prompt path is untested (manual check 6) |
| OSR AltGr on a Swedish layout | `osr-actions.sh` | With `setxkbmap se`, AltGr+2 then `åäö` gave the text field `@åäö` |
| OSR `pdf <path>` | `osr-actions.sh` | `[pdf] ok=true`; the file is a 4-page Letter PDF (`pdfinfo`) |
| OSR `go <url>` | `osr-actions.sh` | `[load-end] https://www.google.com/` |
| OSR `quit` | `osr-actions.sh` | app exit status 0 |
| Chrome: tab opened from Chrome's UI | `chrome-actions.sh` | Ctrl+T, then typing a URL in the omnibox: the new tab got the app's client (`[browser] created id=2 open=2`, `[load-end] https://www.google.com/`) ([screenshot](evidence/chrome-two-tabs.png)) |
| Chrome: close the first tab | `chrome-actions.sh` | Ctrl+1, Ctrl+W: browser 1 closed, the app kept running with one tab |
| Chrome: close the last tab | `chrome-actions.sh` | Ctrl+W: app exit status 0 |
| Restart persistence, OSR | `osr-actions.sh` then `persist-actions.sh` | Second run on the same profile: localStorage load counter 2 (first run: 1); `Default/Cookies` has `.google.com` cookies from the first run; closing the window through the window manager exited with status 0 |
| Restart persistence, Chrome | `chrome-actions.sh` then `persist-actions.sh` | Same as OSR: counter 2, `.google.com` cookies present, window close exited with status 0 |

The first run on each profile ended with `quit` (OSR) or by closing the last tab (Chrome); the second ended with a window close. Restart persistence of a Google sign-in is still manual check 1.

### Isolation note

The first OSR self-check ran under a plain `dbus-run-session`. D-Bus activation on that private bus started a keyring helper that printed `GNOME_KEYRING_CONTROL=/run/user/1000/keyring`, the owner's keyring control directory, and started portal and gvfs services. So that run may have reached the owner's running `gnome-keyring-daemon` through its control socket. The run's logs show no secret request and no unlock prompt, but logs alone cannot rule out that the helper talked to the daemon. Every later run used `scripts/session-bus.conf` (no service directories, so nothing is activated) and a scratch `XDG_RUNTIME_DIR`, and printed no keyring, portal or gvfs lines.

## Manual checklist for the owner

Use a Google account made for this test. Enter its password and MFA code yourself. Nothing in the app logs keystrokes or form contents; the terminal shows only page `console.log` output, load events and permission requests. Do not paste account identifiers into this report.

Use a fresh `--profile` per mode, for example `$HOME/flecha-spike0/osr` and `$HOME/flecha-spike0/chrome`. Start both with `env -u WAYLAND_DISPLAY` as in [Build and run](#build-and-run). Run every check in both modes. Run the same checks in today's Flecha as the reference column.

The links below were checked on 2026-11-23 (HTTP 200; the herokuapp page shows the `tomsmith` / `SuperSecretPassword!` demo login). Any similar page works if one has moved.

0. **Pre-flight: typing on your real display (OSR only).** Xvfb tested keys from xdotool, not your keyboard and input method.
   1. Start OSR mode with `--url file://$PWD/spikes/cef-osr-signin/scripts/input-test.html` and a scratch `--profile`.
   2. In the first text field type `@`, `åäö`, a few AltGr characters from your layout (for example `{[]}\` and `€`), and a capital letter with Shift.
   3. Copy some text from another app and paste it into the field with Ctrl+V.
   4. Pass: the terminal's `[console] INPUT text "…"` lines show exactly what you typed and pasted. If any character is wrong or missing, stop: the sign-in checks would fail for input reasons, not for Alloy reasons.
   5. Also record, without making it a pass condition, whether Ctrl+C in the field and Ctrl+V in another app works (listed below as not implemented).

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
   - Pass: the site reports camera access granted. The machine needs a camera. Without one, OSR mode prints no prompt at all (seen on Xvfb: `NotFoundError`); record "no camera".
7. **Print.** On any page press Ctrl+P.
   - Chrome mode: pass if the print preview opens.
   - OSR mode: record what Ctrl+P does (on Xvfb it did nothing visible). Then type `pdf` and check that `<profile>/flecha-spike-print.pdf` contains the page. The plan expects "Use `Print` or a GPUI preview over `PrintToPDF`".

### Results (fill in; pass, fail or n/a, plus a short note)

| # | Check | Alloy OSR | Chrome-style control | Flecha (reference) |
|---|---|---|---|---|
| 0 | Typing and paste on the real display | | n/a | n/a |
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

- **Google sign-in, Web Store installs, password save and fill, camera, print preview.** All need a person, an account or hardware. None were attempted. No account was signed in to.
- **The camera prompt in OSR mode.** Xvfb has no camera, so the request failed before any permission callback. The notification prompt shows the terminal prompt path works, but the media path (`OnRequestMediaAccessPermission`) never ran.
- **Chrome-mode permission bubbles and print preview.** Not scripted; they are Chrome's own UI. Manual checks 6 and 7.
- **Clipboard and the owner's real keyboard.** Paste was not tested. AltGr was tested only with xdotool on an Xvfb Swedish layout. Pre-flight check 0 covers both.
- **Native Wayland.** Every run used Xvfb (X11). With `WAYLAND_DISPLAY` set on the owner's session, winit would pick Wayland for the OSR window, and CEF picks its own Ozone platform. Neither was run; the run commands unset `WAYLAND_DISPLAY`.
- **HiDPI.** Xvfb runs at scale 1. The OSR code passes the window scale to CEF (`GetScreenInfo`) and converts mouse coordinates, but no scale other than 1 was tested.
- **OSR features this spike does not implement:** IME composition, context menus, drag and drop, JavaScript `alert`/`confirm`/`beforeunload` dialogs, file pickers, clipboard to other apps, DevTools, an address bar. If a sign-in page depends on one of these, the failure belongs to this spike, not to Alloy. Note it.
- **A lost first click in OSR.** In the first scripted run, a click sent right after the page's first `load-end` reached nothing (no click event on the page). After a 0.5 s pause it worked, in every later run. I did not find the cause. If a click right after a page load, or right after switching back from the terminal, does nothing, note it under check 0.
- **The owner's login keyring.** Every automated run used a private D-Bus bus with no keyring, so CEF stored cookies and passwords without one. On the owner's desktop session, manual checks 1 and 5 will likely go through the login keyring (Chromium's Linux password and cookie encryption) and may add an entry to it.
- **Keys with no US physical-key mapping in OSR.** Text from them is inserted correctly through `CHAR` events, but the page's `keydown` sees `key` as a NUL character and `keyCode` 0 (seen with xdotool's remapped `ö`, [log](evidence/osr-input-events.txt)). A real Swedish layout maps `ö` to a physical key, so this is expected to matter only for unusual layouts.
- **Beta archive.** The build uses the stable archive of the pinned commit, not the beta archive (pins.toml SHA-1 `057a0b87…`).
