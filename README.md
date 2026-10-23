# Flecha

An Arc-style browser for developers, built on a Chromium fork. Arc's interface ideas are rebuilt from scratch; no Arc code is used.

## Why

Arc's vertical sidebar, Spaces and Peek are hard to give up, and Arc is no longer where the work is happening. Flecha is a replacement that keeps those ideas and adds tools for the people who build websites.

It is a Chromium 158 fork rather than an Electron app because Google blocks sign-in in Electron. Flecha is real Chromium, so Google sign-in, Chrome Web Store extensions and the rest of the web platform work as they do in Chrome. Flecha started as an Electron app; that version is gone from the repo (it's in the git history before the move to `chromium/`).

## Features

**Sidebar and tabs**
- A vertical sidebar with favourites, pinned tabs (with folders) and Today tabs, per Space
- Drag tabs between favourites, pinned, folders and Today
- Today tabs archive themselves after 12 hours by default (1 day, 7 days, 30 days or never in Settings); the archive is searchable
- Closing a pinned tab or favourite only unloads it and sends it back to its pinned URL
- Per-site favicons, tab muting, duplicate, copy link or Markdown link, copy screenshot
- On launch, the window comes back where it was

**Spaces**
- Spaces with their own colour and emoji; in dark mode the colour becomes a deep tint of the same hue
- Right-click a Space, then Separate Cookies and Logins, to be signed in as different users side by side (a persistent storage partition per Space)
- Switch with ⌃1 to ⌃9, or next and previous Space with ⌥⌘← and ⌥⌘→

**Command bar**
- ⌘T for a new tab, ⌘L for the current one; it searches open tabs, the archive and history
- Developer shortcuts: `gh owner/repo`, `gh 123` (in the current tab's repo), `npm react`, `mdn grid`, `caniuse :has`, `so ...`, and `env`
- Paste a JWT or a Unix timestamp to decode it in place
- Type `boost` to open the Boosts page

**Peek, split view and Flechinha**
- Peek: links that open a new tab from a pinned tab or favourite show in a floating preview; any link's right-click menu offers it too. Esc or a click outside closes it, Open turns it into a tab
- Split view: right-click a tab, Open in Split View, or ⌃⇧= to split with a new tab (⌃⇧- exits)
- Flechinha: a small standalone window for a quick page (⌥⌘N), which can be sent back into the main window

**Boosts**
- Per-site TypeScript and CSS (`github.com.ts`, `github.com.css`, `all.css`), kept in Flecha and edited on the Boosts page. CSS applies live; scripts reload the page. ⌥⌘B edits the current site's boost

**For developers**
- Console error badge: local tabs show how many console errors they have; clicking it opens the Console
- Responsive view (⌥⌘M): the page as a phone (393px, touch, mobile user agent) next to desktop
- Running locally: dev servers listening on this Mac show up in the sidebar, named after their project folder
- Environments: group a project's origins (local, staging, production) in Settings; ⌥⌘E moves the page to the next one with the same path, and the address bar shows a LOCAL, STAGING or PROD badge
- Chromium's DevTools (⌥⌘I), View source (⌥⌘U), zoom and Find in page
- Coding agents (MCP): Settings, Coding agents turns on a local MCP server (127.0.0.1, token required) so Claude Code, Cursor and others can list tabs, read pages and console logs, take screenshots, click, type and run JavaScript. It needs the native host (see Install)

**Privacy and the rest of the browser**
- Built-in ad and tracker blocking (EasyList and EasyPrivacy as declarativeNetRequest rules, plus element hiding), toggled in Settings
- Incognito windows get the same Flecha frame, with one in-memory Space that is forgotten when the last incognito window closes
- Import bookmarks and history from other browsers and from Arc's sidebar (needs the native host)
- Chrome extensions: install from the Chrome Web Store; their buttons appear under the address bar
- Light, dark or system theme; every shortcut can be re-recorded in Settings (⌘/ lists them)
- Settings (⌘,): search engine (Google, DuckDuckGo, Bing, Brave, Kagi, Ecosia), ad blocking, default browser, Peek behaviour, shortcuts, environments

Not there yet in the Chromium build: choosing the downloads folder from Settings, the welcome tour, and a resizable split divider.

