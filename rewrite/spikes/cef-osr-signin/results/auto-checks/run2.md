# Spike 0 automated checks

Written by `scripts/auto-checks.sh` (modes: osr chrome flecha). Every cell was observed on Xvfb through `scripts/xvfb-isolated.sh`; its label says how. Checks 1 to 3 need a person and a Google account and are never automated.

## Build stamp

| Item | Value |
|---|---|
| Date (UTC) | 2026-11-23T19:11:17Z |
| Commit | `75387521bc4f029bf706124279807aa8d98e6a55`, clean tree |
| Spike binary sha256 | `1b67ea5e01972dd9ab7e7200ef35c3c6390bb2d1901eaeba5250daf18288c978` |
| CEF pin | `158.0.3+g8c29be2+chromium-158.0.8127.18 beta` |
| Flecha | `Flecha 158.0.8127.2`, chrome sha256 `38d13a6c5c539a40…` |
| Password store | `--password-store=basic` in a scratch HOME on every run; no keyring is reachable (no session bus but the private one, system bus address points nowhere) |
| WAIT_SECS | 30 (per-step wait; a test bound) |
| Tools | xdotool version 4.20260303.1, xclip 0.13, xorg-server-xvfb 21.1.26-1.1, ImageMagick 7.1.2-36, pdftotext version 26.11.0 |

## Results

| # | Check | Alloy OSR | Chrome-style control | Flecha (reference) |
|---|---|---|---|---|
| 0 | Typing and paste (Xvfb, xdotool; the owner's real keyboard, IME and Wayland stay manual) | typing pass: auto, Xvfb, xdotool with setxkbmap se: AltGr+2, å, ä, ö, AltGr+7/8/9/0/+, AltGr+e, Shift+A gave the field exactly @åäö{[]}\€A; paste fail: auto, Xvfb: no pasted text reached the field within 30s of Ctrl+V (clipboard owner: xclip on the same display) | typing pass: auto, Xvfb, xdotool with setxkbmap se: AltGr+2, å, ä, ö, AltGr+7/8/9/0/+, AltGr+e, Shift+A gave the field exactly @åäö{[]}\€A; paste pass: auto, Xvfb: Ctrl+V pasted text owned by xclip on the same display | typing pass: auto, Xvfb, xdotool with setxkbmap se: AltGr+2, å, ä, ö, AltGr+7/8/9/0/+, AltGr+e, Shift+A gave the field exactly @åäö{[]}\€A; paste pass: auto, Xvfb: Ctrl+V pasted text owned by xclip on the same display |
| 1 | Gmail sign-in with MFA, survives restart | manual | manual | manual |
| 2 | Sign out and back in | manual | manual | manual |
| 3 | Third-party "Sign in with Google" | manual | manual | manual |
| 4a | Web Store: "Add to Chrome" enabled and install completes | fail: auto, Xvfb, not signed in, probe: button add, enabled true; real click on the button (the probe reported the trusted click: no); the browser process died with no message on stderr (REPORT has the core dump's stack) | pass: auto, Xvfb, not signed in, probe: button add, enabled true; Return on the focused button (1 presses), install dialog took focus 1 times, Tab and Return on it; Dark Reader unpacked in the profile | not automated: Flecha has no page probe (the spike's --probe), which this step needs to find and focus the store button |
| 4b | Extension content script runs | not reached: the app died in 4a, so 4b was not tried in this run (auto-checks.sh tries OSR's 4b again in content-osr); then fail: auto, Xvfb screenshot, on a copy of the Chrome-style profile with Dark Reader installed; example.com loaded: yes; window title: Flecha spike: osr; page background grey level 238 of 255 (dark below 128); probe: body rgba(0, 0, 0, 0), html rgb(238, 238, 238) | pass: auto, Xvfb screenshot, example.com loaded: yes; window title: Example Domain - Chromium; page background grey level 26 of 255 (dark below 128); probe: body rgb(24, 26, 27), html rgb(24, 26, 27) | not automated: needs Dark Reader installed, and 4a is not automated here (REPORT says what was tried) |
| 4c | Extension popup opens | n/a: auto, Xvfb: Alloy OSR has no toolbar or other surface to open an extension popup | not automated: the popup opens from the toolbar's extension menu, Chrome's own UI | not automated: the popup opens from the toolbar's extension menu, Chrome's own UI |
| 5a | Password save offered | fail: auto, Xvfb, --password-store=basic in a scratch HOME: logged in with the public demo login; Alloy has no save prompt and no save callback to answer; Login Data holds 0 logins for the demo site after 30s | pass: auto, Xvfb, --password-store=basic in a scratch HOME: logged in with the public demo login; Alt+Shift+A focused the save bubble, Return pressed Save; Login Data holds 1 login for the demo site | not automated: Flecha has no page probe (the spike's --probe), which this step needs to focus the login fields and see the secure page |
| 5b | Saved password fills | pass: auto, Xvfb, --password-store=basic in a scratch HOME, 1 saved login(s) in the profile (a copy of the Chrome-style profile that saved it): after a real click the fields hold user 8 and password 20 characters (before the click: user 0, password 0 characters) | pass: auto, Xvfb, --password-store=basic in a scratch HOME, 1 saved login(s) in the profile: after a real click the fields hold user 8 and password 20 characters (before the click: user 0, password 0 characters) | not automated: Flecha has no page probe (the spike's --probe), which this step needs to read the login fields' lengths |
| 6 | Camera permission through a host prompt (fake device) | pass: auto, Xvfb, fake camera device: prompt: [permission 1] answered with allow 1; the page got one video track | pass: auto, Xvfb, keyboard only, fake camera device: Tab to the button, Space; the permission bubble took focus; Tab Tab Tab, Return on "Allow while visiting the site"; the page got one video track | pass: auto, Xvfb, keyboard only, fake camera device: Tab to the button, Space; the permission bubble took focus; Tab Tab Tab, Return on "Allow while visiting the site"; the page got one video track |
| 7 | Print | pass: auto, Xvfb: Ctrl+P logged [print] start (the spike has no print dialog or preview); the pdf command wrote a PDF containing the page text | pass: auto, Xvfb, keyboard only: Ctrl+P opened the print preview; Return (Save) and a typed path in the Save File dialog wrote a PDF containing the page text | pass: auto, Xvfb, keyboard only: Ctrl+P opened the print preview; Return (Save) and a typed path in the Save File dialog wrote a PDF containing the page text |

## Fake-camera guard

| Run | Result |
|---|---|
| guard-cli (spike) | pass: https://accounts.google.com/ refused before anything started (exit 2, no profile created) |
| guard-cli (flecha) | pass: https://accounts.google.com/ refused before anything started (exit 2, no profile created) |
| guard-cli (spike) | pass: https://mail.google.com/ refused before anything started (exit 2, no profile created) |
| guard-cli (flecha) | pass: https://mail.google.com/ refused before anything started (exit 2, no profile created) |
| guard-cli (spike) | pass: https://myaccount.google.com/ refused before anything started (exit 2, no profile created) |
| guard-cli (flecha) | pass: https://myaccount.google.com/ refused before anything started (exit 2, no profile created) |
| guard-cli (spike) | pass: https://accounts.youtube.com/ refused before anything started (exit 2, no profile created) |
| guard-cli (flecha) | pass: https://accounts.youtube.com/ refused before anything started (exit 2, no profile created) |
| guard-cli (spike) | pass: https://example.com/ refused before anything started (exit 2, no profile created) |
| guard-cli (flecha) | pass: https://example.com/ refused before anything started (exit 2, no profile created) |
| guard-osr (osr) | pass: auto, Xvfb, --fake-camera: 0 requests reached 127.0.0.2 (frame, image, fetch, redirected fetch, navigation, redirected navigation all tried); 7 [blocked] lines; 2 go commands to Google sign-in origins refused |
| guard-osr-control (osr) | pass: auto, Xvfb, no --fake-camera: all six requests reached 127.0.0.2, so the guarded run's 0 is the guard's doing |
| guard-chrome (chrome) | pass: auto, Xvfb, --fake-camera: 0 requests reached 127.0.0.2 (frame, image, fetch, redirected fetch, navigation, redirected navigation all tried); 8 [blocked] lines; 0 go commands to Google sign-in origins refused |
| guard-chrome-control (chrome) | pass: auto, Xvfb, no --fake-camera: all six requests reached 127.0.0.2, so the guarded run's 0 is the guard's doing |

## Negative controls

Each control runs the same pass condition with its cause removed. A control that passes would mean the check can pass when it should not; none may.

| Control run | Row | Result |
|---|---|---|
| input-osr-control | 0a | fail: auto, Xvfb, xdotool with setxkbmap se: the field did not report @åäö{[]}\€A |
| input-osr-control | 0b | fail: auto, Xvfb: no pasted text reached the field within 30s of Ctrl+V (clipboard owner: none) |
| camera-osr-control | 6 | fail: auto, Xvfb, no camera device: prompt: none; page result: camera error NotFoundError |
| print-osr-control | 7 | fail: auto, Xvfb: [print] start after Ctrl+P: no; PDF from the pdf command contains the page text: no |
| store-osr-control | 4a | fail: auto, Xvfb, not signed in, probe: button add, enabled true; nothing pressed (control); no Dark Reader in the profile within 30s |
| store-osr-control | 4b | fail: auto, Xvfb screenshot, example.com loaded: yes; window title: Flecha spike: osr; page background grey level 238 of 255 (dark below 128); probe: body rgba(0, 0, 0, 0), html rgb(238, 238, 238) |
| input-chrome-control | 0a | fail: auto, Xvfb, xdotool with setxkbmap se: the field did not report @åäö{[]}\€A |
| input-chrome-control | 0b | fail: auto, Xvfb: no pasted text reached the field within 30s of Ctrl+V (clipboard owner: none) |
| camera-chrome-control | 6 | fail: auto, Xvfb, keyboard only, no camera device: Tab to the button, Space; no prompt took focus; page result: camera error NotFoundError |
| print-chrome-control | 7 | fail: auto, Xvfb, keyboard only: no PDF with the page text after Ctrl+P, Return and the Save File dialog |
| store-chrome-control | 4a | fail: auto, Xvfb, not signed in, probe: button add, enabled true; nothing pressed (control); no Dark Reader in the profile within 30s |
| store-chrome-control | 4b | fail: auto, Xvfb screenshot, example.com loaded: yes; window title: Example Domain - Chromium; page background grey level 238 of 255 (dark below 128); probe: body rgba(0, 0, 0, 0), html rgb(238, 238, 238) |
| input-flecha-control | 0a | fail: auto, Xvfb, xdotool with setxkbmap se: the field did not report @åäö{[]}\€A |
| input-flecha-control | 0b | fail: auto, Xvfb: no pasted text reached the field within 30s of Ctrl+V (clipboard owner: none) |
| camera-flecha-control | 6 | fail: auto, Xvfb, keyboard only, no camera device: Tab to the button, Space; no prompt took focus; page result: camera error NotFoundError |
| print-flecha-control | 7 | fail: auto, Xvfb, keyboard only: no PDF with the page text after Ctrl+P, Return and the Save File dialog |
| password-save-osr-control | 5a | fail: auto, Xvfb, --password-store=basic in a scratch HOME: logged in with the public demo login; Alloy has no save prompt and no save callback to answer; Login Data holds 0 logins for the demo site after 30s |
| password-save-chrome-control | 5a | fail: auto, Xvfb, --password-store=basic in a scratch HOME: logged in with the public demo login; save bubble left unanswered (control); Login Data holds 0 logins for the demo site after 30s |
| password-fill-osr-control | 5b | fail: auto, Xvfb, --password-store=basic in a scratch HOME, 0 saved login(s) in the profile: the fields stayed empty for 30s after a real click (before the click: user 0, password 0 characters) |
| password-fill-chrome-control | 5b | fail: auto, Xvfb, --password-store=basic in a scratch HOME, 0 saved login(s) in the profile: the fields stayed empty for 30s after a real click (before the click: user 0, password 0 characters) |
| empty PDF file | 7 | did not pass |
