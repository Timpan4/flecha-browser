# Flecha

An Arc-style browser built on Chromium (through Electron). Arc's interface ideas are rebuilt from scratch; no Arc code is used.

```bash
npm install
npm start
```

For a throwaway profile: `FLECHA_USER_DATA=/tmp/flecha-dev npm start`.

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
- Links from other apps open in a new tab, and launching Flecha again reuses the open window. Making Flecha the default browser only works in a packaged Flecha.app; from `npm start` the button is disabled because macOS would register the generic Electron binary

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
- Built-in ad and tracker blocking (EasyList, EasyPrivacy, uBlock lists), toggled in Settings

### Extension caveats

- Electron exposes `declarativeNetRequest` but never enforces it, so MV3 blockers like uBlock Origin Lite install and show their popup but block nothing. That's why blocking is built in.
- Native messaging is not available, so extensions that talk to a desktop app (1Password's app integration, for example) can't reach it.
- Extension support comes from `electron-chrome-extensions`, licensed GPL-3.0 here. Distributing Flecha means distributing it under the GPL, unless you buy that library's commercial licence.

## Layout

- `src/main/main.ts`: window, one `WebContentsView` per tab (created lazily), menus, IPC, permission prompts
- `src/main/store.ts`: the sidebar model and persistence (`flecha-state.json` in the user data folder)
- `src/main/arcImport.ts`: Arc sidebar parser
- `src/main/extensions.ts`, `src/preload/extension-compat.ts`: Chrome extension and Web Store support
- `src/main/adblock.ts`: built-in ad blocker
- `src/main/history.ts`, `src/main/downloads.ts`: browsing history and downloads
- `src/main/settings.ts`, `src/renderer/settings.*`, `src/preload/settings-preload.ts`: the settings window
- `src/main/defaultBrowser.ts`: links from other apps and the default-browser setting
- `src/main/agentBridge.ts`: the MCP server for coding agents
- `src/main/environments.ts`, `src/main/localServers.ts`, `src/main/sessions.ts`, `src/main/boosts.ts`: environments, local dev servers, per-Space sessions and boosts
- `src/renderer/devCommands.ts`: command bar shortcuts
- `src/renderer/find.*`, `src/preload/find-preload.ts`: the find-in-page bar
- `src/renderer/`: sidebar and command bar UI
- `src/preload/preload.ts`: the bridge between the sidebar UI and the main process

## Next up

Resizable split divider and 3–4 pane splits, Little Arc–style quick windows,
and packaging with electron-builder.
