import { contextBridge } from 'electron';

/**
 * Recent Chromium gives extensions a native `browser` namespace next to `chrome`.
 * electron-chrome-extensions only patches `chrome` (tabs, windows, commands, action…), so
 * `browser` is left with Electron's bare versions: some APIs are missing outright and
 * others, like browser.tabs.query, exist but return nothing. Extensions that prefer
 * `browser` (uBlock Origin Lite, 1Password) then crash or hang, e.g. 1Password's popup
 * waits forever for the current tab.
 *
 * This makes every `browser` API the same object as its `chrome` counterpart. It is
 * registered after the library's own preload, so `chrome` is already filled in.
 */
function mirrorChromeOntoBrowser() {
  const g = globalThis as { chrome?: Record<string, unknown>; browser?: Record<string, unknown> };
  if (!g.chrome || !g.browser || g.browser === g.chrome) return;
  for (const key of Object.keys(g.chrome)) {
    if (g.browser[key] === g.chrome[key]) continue;
    try {
      Object.defineProperty(g.browser, key, { value: g.chrome[key], configurable: true, writable: true, enumerable: true });
    } catch {
      // A non-configurable native property; the extension still has `chrome` for it.
    }
  }
}

const href: string = (globalThis as { location?: { href: string } }).location?.href ?? '';
if (process.type === 'service-worker' || href.startsWith('chrome-extension://')) {
  contextBridge.executeInMainWorld({ func: mirrorChromeOntoBrowser });
}
