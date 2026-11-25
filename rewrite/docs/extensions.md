# Extensions on stock CEF

Flecha uses the Linux managed policy provider in CEF `158.0.3+g8c29be2` (Chromium `158.0.8127.18`) for Chrome Web Store installations. Open **Settings → Extensions**, enter a Store link or ID, review the permissions, and confirm the installation. Flecha reads the manifest from the Store package before adding the ID to `<data-dir>/extension-policy/managed/flecha.json`. Chromium verifies the package when it installs through `ExtensionInstallForcelist`. The policy names only the ID, so Chromium installs the version the Store serves at restart, which can be newer than the reviewed one. A Store package that changes between review and restart is not checked again. Managed installation grants required permissions automatically, including changes in later updates. Remove an extension in Settings to take it out of the policy. Restart Flecha to apply managed changes. A fresh running profile wrote the policy successfully but did not install within the slice harness's 10-second step allowance.

Under **Developer mode**, select a folder containing an MV3 `manifest.json`. Flecha saves the folder list in `<data-dir>/extensions.json` and passes it to `--load-extension` on startup. Restart after adding or removing a folder. Managed Store installation uses the Linux policy path. macOS and Windows managed installation are outside this implementation.

The sidebar's puzzle button opens the extensions menu. Pin actions there to show their icons below the address bar. The inventory uses the CEF profile's extension preferences, packed manifests under `Extensions/<id>/<version>/`, and the manifests in configured unpacked folders. Popup actions open their declared `chrome-extension://` page in a separate Alloy OSR browser anchored to the action or menu. Settings flags actions that need tab access and actions that have no popup. The shell closes the popup when the selected tab changes.

## Differences from the Chromium build

- Stock CEF exposes neither the toolbar action runner nor the active browser window model that extension tab queries depend on. Popups use their manifest's default page. Per-tab icons, badges, dynamic popups, per-site access controls, `activeTab` grants, and native `onClicked` actions need the Chromium toolbar bridge. The context menu in Flecha opens its own extension list with pinning and a Settings link.
- The GPUI popover fits within the window and uses Chromium's popup size ceiling. It uses a regular Alloy page for the popup, so Chrome toolbar popup lifecycle and native preferred sizing differ.
- All persistent Spaces receive the managed and unpacked lists. Shared Spaces use the same extension storage. A Space with separate cookies gets its own install and extension storage. A new persistent Space begins policy installation when its CEF context initializes.
- Private windows use off-the-record contexts, and Flecha never enables extensions there.
- Agent tabs share their Space's context, so they keep the user's sign-ins, and that Space's extensions run in them. The Chromium build kept the session too, but patch 0080 also skipped other extensions' content scripts and `executeScript()` in agent tabs. Stock CEF has no per-tab injection filter, so that part waits for the CEF fork.

## Verification

Run from `rewrite/` with the debug build and pinned CEF runtime staged.

```sh
tools/verify-slice.sh extensions
tools/verify-slice.sh store-extensions
```

`extensions` uses Xvfb, a private session bus, and a loopback-only network namespace. It creates a signed test CRX, checks a no-extension control, opens the sidebar action and its popup through X11 input, and checks that agent tabs share the user's cookies and extension, that private windows get neither, managed removal, and unpacked startup loading.

`store-extensions` uses the same isolation with outbound networking provided by `pasta`. It disables port forwarding and host loopback mapping. The launcher checks that it has a different network namespace and no host runtime socket mounts before starting Xvfb and the private bus. The test uses scratch profiles and stops at Bitwarden's signed-out screen.

## Chrome Web Store results

Measured on Linux/Xvfb on 2026-11-25. The app displayed each extension's permissions before it wrote the managed entry. All three installed from Google's signed CRXs after restart. The initial CEF probe got HTTP 200 from `https://clients2.google.com/service/update2/crx`, then fetched the CRXs from `clients2.googleusercontent.com`. All three installed manifests list the same Google update URL, and each popup's `chrome.runtime.requestUpdateCheck()` returned `no_update`. Installing a newer published version over an older one was unverified.

| Extension | Installed version | Content scripts and page result | Popup and remaining gap |
| --- | --- | --- | --- |
| uBlock Origin Lite | `2026.1124.1811` | Executed its CSS API, EasyList, EasyPrivacy, and other registered scripts on the loopback page. An ad script request loaded before install and failed with the extensions installed. | `/popup.html` painted and reported “not a website.” The active-tab query returned zero tabs, so the popup could not show the site's controls. |
| Bitwarden | `2026.11.0` | Executed its content message handler and autofill injection trigger. | `/popup/index.html` reached the signed-out **Create account / Log in** screen. The active-tab query returned zero tabs. Vault access and autofill with an account were untested. |
| Dark Reader | `4.9.135` | Executed `inject/proxy.js`, `inject/fallback.js`, `inject/index.js`, and `inject/color-scheme-watcher.js`. The page had no `data-darkreader-mode` marker. | `/ui/popup/index.html` painted its controls and treated the active page as `about:`, a protected page. The active-tab query returned zero tabs. The earlier CEF probe also logged a “No current window” error from its background worker. |

The verbose CEF probe emitted `*** stack smashing detected ***: terminated` during and after fresh downloads. Installs succeeded. The product harness emitted no stack-smashing messages, and the probe stderr did not identify the emitting process.
