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
- Chrome extensions: install from the Chrome Web Store (Extensions → Chrome Web Store, then "Add to Chrome"); their buttons appear under the address bar. Manage or remove them from the Extensions menu
- Built-in ad and tracker blocking (EasyList, EasyPrivacy, uBlock lists), toggled in Extensions → Block Ads and Trackers

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
- `src/renderer/find.*`, `src/preload/find-preload.ts`: the find-in-page bar
- `src/renderer/`: sidebar and command bar UI
- `src/preload/preload.ts`: the bridge between the sidebar UI and the main process

## Next up

Resizable split divider and 3–4 pane splits, Little Arc–style quick windows,
and packaging with electron-builder.
