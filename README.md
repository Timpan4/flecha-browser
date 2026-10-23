# Flecha

An Arc-style browser built on a Chromium fork. Arc's interface ideas are rebuilt from scratch; no Arc code is used.

```bash
npm ci                                   # esbuild, TypeScript and the shell's libraries
npm run shell:build                      # the shell extension, into chromium/shell/dist (shell:watch rebuilds on save)
chromium/scripts/install-native-host.sh  # builds and registers the native messaging host
chromium/scripts/build.sh                # patches the Chromium checkout and builds out/Dev/Flecha.app
chromium/scripts/run.sh                  # launches it with its own profile and the shell from chromium/shell/dist
npm run typecheck
```

`chromium/scripts/env.sh` says where the Chromium checkout lives. Shell changes need no Chromium build: rebuild the shell and relaunch. `chromium/scripts/build-release.sh` makes the release app and `package-release.sh` signs it and makes the dmg.

Flecha started as an Electron app; that version is gone from the repo (it's in the git history before the move to `chromium/`).

## What works

- Vertical sidebar with favorites, pinned tabs (with folders) and today tabs, per Space
- Spaces with their own colour, switched with ⌃1–⌃9; rename by double-clicking the space name
- Command bar (⌘T new tab, ⌘L current tab) that searches open tabs and the archive
- Drag tabs between favorites, pinned, folders and today
- Today tabs archive themselves after 12 hours; ⇧⌘K clears them; ⇧⌘T reopens the last one
- Closing a pinned tab or favorite only unloads it and sends it back to its pinned URL
- File → Import from Arc… reads `~/Library/Application Support/Arc/StorableSidebar.json`
- ⌘S hides the sidebar; ⌘D pins or unpins; ⇧⌘C copies the URL
- Split view: right-click a tab → Open in Split View, or ⌃⇧= to split with a new tab (⌃⇧- exits). Clicking into a pane makes it the active tab
- Peek: links that open a new tab from a pinned tab or favorite open in a floating preview instead; also available from any link's right-click menu. Esc or a click outside closes it, Open turns it into a tab
- History: every page you visit is searchable from the command bar
- Downloads save straight to ~/Downloads (no dialog) and are listed from the sidebar's download button
- Find in page with ⌘F, ⌘G and ⇧⌘G
- On launch, the window returns to its last position and each tab keeps its back/forward history and scroll position
- Settings (⌘,): search engine (Google, DuckDuckGo, Bing, Brave, Kagi, Ecosia), downloads folder and "ask where to save", ad blocking, default browser
- Links from other apps open in a new tab, and launching Flecha again reuses the open window

### For developers

- **Coding agents (MCP):** Settings → Coding agents turns on a local MCP server (127.0.0.1, token required) so Claude Code, Cursor and others can list tabs, read pages and console logs, take screenshots, click, type and run JavaScript. The sidebar shows what the agent is doing. Settings copies the `claude mcp add` command for you
- **Environments:** group a project's origins (local, staging, production) in Settings; ⌥⌘E moves the page to the next one with the same path, and the address bar shows a LOCAL / STAGING / PROD badge
- **Running locally:** dev servers listening on this Mac show up in the sidebar, named after their project folder
- **Console error badge:** local tabs show how many console errors they have; clicking it opens the Console
- **Separate cookies per Space:** right-click a Space → Separate Cookies and Logins, to be signed in as different users side by side
- **Responsive view (⌥⌘M):** the page as a phone (393px, touch, mobile user agent) next to desktop, following its navigation
- **Boosts:** per-site TypeScript and CSS in `~/.flecha/boosts` (`github.com.ts`, `github.com.css`, `all.css`), compiled with esbuild; CSS applies live, scripts reload the page. Type `boost` in the command bar or press ⌥⌘B
- **Command bar shortcuts:** `gh 123`, `gh owner/repo`, `npm react`, `mdn grid`, `caniuse :has`, `so …`, `env`; paste a JWT or a Unix timestamp to decode it

- Chrome extensions: install from the Chrome Web Store (Extensions → Chrome Web Store, then "Add to Chrome"); their buttons appear under the address bar. Manage or remove them from the Extensions menu
- Built-in ad and tracker blocking (EasyList, EasyPrivacy), toggled in Settings

## Layout

- `chromium/patches/`, `chromium/branding/`, `chromium/build/`: the changes to Chromium, its branding and the GN args
- `chromium/scripts/`: apply the patches, build, run, install the native host, build and package a release
- `chromium/shell/`: the shell extension (MV3), which draws the sidebar, command bar and settings; built by `build.mjs`
  - `background.ts`: the service worker that owns the Store and the tabs; `features/`: ad blocking, boosts, console errors, local servers, responsive view
  - `ui/`: the sidebar, command bar, settings and Flechinha pages (TypeScript, HTML, CSS), on `flechaApi.ts`, `settingsApi.ts` and `flechinhaApi.ts`
  - `lib/`: URLs and search engines, shortcuts, environments, settings rules, the tab manager, boost names
- `chromium/native-host/`: the native messaging host (Node): importing bookmarks and history from other browsers and Arc, reading the Electron version's saved state, the MCP server for coding agents; built by `build.mjs`
- `chromium/shared/`: what the shell and the native host share: the types, the sidebar model (`store.ts`), import and agent tool definitions

## Next up

Resizable split divider and 3–4 pane splits.
