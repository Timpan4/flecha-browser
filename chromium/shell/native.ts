/**
 * The shell's side of Flecha's native patches (0016+): what chrome.flechaPrivate does beyond
 * the window frame. background.ts calls registerNative(ctx) once, after the Store is loaded.
 *
 * Every call checks that the API is there, so the shell still runs on a Chromium without the
 * patch (stock Chromium, or a build older than the shell).
 */
import type { Store } from '../../src/main/store';
import { SHORTCUTS, keysFor } from '../../src/main/shortcuts';
import { handle } from './hub';

export interface NativeContext {
  store: Store;
  /**
   * Runs one of Flecha's commands, by id: every id in src/main/shortcuts.ts (newTab,
   * toggleSidebar, …), plus `selectTab:1`…`selectTab:9` (⌘1…⌘9; 9 is the last tab) and
   * `selectSpace:1`…`selectSpace:9` (⌃1…⌃9). `windowId` is the window the key was pressed in.
   */
  run(command: string, windowId?: number): void;
  /** The Chromium tab of an open Flecha tab (background.ts's `live` map). */
  chromeTabId?(flechaId: string): number | undefined;
  /** The Flecha tab of a Chromium tab. */
  flechaTabId?(chromeId: number): string | undefined;
  /**
   * Forgets a Flecha tab's Chromium tab without closing it (it moved into a Flechinha), then
   * closes the Flecha tab. background.ts: `live.delete(id); rememberLive(); store.closeTab(id)`.
   */
  releaseTab?(flechaId: string): void;
  /**
   * A Chromium tab that moved into the main window ("Open in Flecha" from a Flechinha)
   * becomes an active Today tab in the active Space, like onCreated does for new tabs.
   */
  adoptTab?(chromeId: number): void;
}

const api = () => (chrome as { flechaPrivate?: typeof chrome.flechaPrivate }).flechaPrivate;
const has = <K extends keyof typeof chrome.flechaPrivate>(name: K) => typeof api()?.[name] === 'function';

// Events go on in the module's first turn, or Chromium drops the one that woke the
// background; they wait here until registerNative() hands over the context.
let resolveCtx!: (ctx: NativeContext) => void;
const ready = new Promise<NativeContext>((resolve) => (resolveCtx = resolve));
const when =
  <A extends unknown[]>(fn: (ctx: NativeContext, ...args: A) => void) =>
  (...args: A) =>
    void ready.then((ctx) => fn(ctx, ...args));

if (has('setShortcuts')) chrome.flechaPrivate.onShortcut.addListener(when((ctx, command: string, windowId: number) => ctx.run(command, windowId)));

export function registerNative(ctx: NativeContext) {
  registerShortcuts(ctx);
  registerTheme(ctx);
  registerSplitRatio(ctx);
  registerFlechinha(ctx);
  resolveCtx(ctx);
}

// ---- Flechinha (patch 0023) --------------------------------------------------------------

/**
 * Flechinha, the small window for one page (src/main/flechinha.ts). Its frame page is
 * window.html?window=<id>&kind=flechinha. Hub commands:
 *  - newFlechinha(): an empty one (the new tab page), ⌥⌘N.
 *  - openFlechinha(url?, tabId?): with a Flecha tab id, that tab's page moves into it and the
 *    Flecha tab closes; otherwise it opens `url`.
 *  - flechinha:openInFlecha(windowId): its page moves back as an active Today tab.
 * Windows that are Flechinhas aren't Flecha's: isFlechinha(windowId) tells them apart.
 */
const flechinhas = new Set<number>();
/** Whether a window is a Flechinha, from what this module has seen (synchronous, may lag). */
export const isFlechinha = (windowId: number | undefined) => windowId !== undefined && flechinhas.has(windowId);
/**
 * Whether a window is a Flechinha, asking the browser. Use this in tabs.onCreated: the event
 * for a new Flechinha's first tab arrives before openFlechinha() resolves.
 */
export async function isFlechinhaWindow(windowId: number | undefined) {
  if (windowId === undefined || !has('getFlechinhas')) return false;
  if (flechinhas.has(windowId)) return true;
  return (await chrome.flechaPrivate.getFlechinhas()).includes(windowId);
}

function registerFlechinha(ctx: NativeContext) {
  if (!has('openFlechinha')) return;
  // Links from other apps (Flecha as the default browser) go to a Flechinha unless turned off.
  let linksSent: boolean | undefined;
  const applyLinks = () => {
    const enabled = ctx.store.state.openLinksInFlechinha !== false;
    if (enabled === linksSent) return;
    linksSent = enabled;
    void chrome.flechaPrivate.setOpenLinksInFlechinha(enabled);
  };
  applyLinks();
  ctx.store.onChange(applyLinks);
  void chrome.flechaPrivate.getFlechinhas().then((ids) => ids.forEach((id) => flechinhas.add(id)));
  chrome.windows.onRemoved.addListener((id) => flechinhas.delete(id));

  async function open(url?: string, flechaId?: string) {
    const tabId = flechaId !== undefined ? ctx.chromeTabId?.(flechaId) : undefined;
    const windowId = await chrome.flechaPrivate.openFlechinha(tabId !== undefined ? { tabId } : url ? { url } : {});
    flechinhas.add(windowId);
    if (tabId !== undefined && flechaId !== undefined) ctx.releaseTab?.(flechaId);
    return windowId;
  }

  handle({
    newFlechinha: () => open(),
    openFlechinha: (url?: string, tabId?: string) => open(typeof url === 'string' ? url : undefined, typeof tabId === 'string' ? tabId : undefined),
    'flechinha:openInFlecha': async (windowId: number) => {
      const tabId = await chrome.flechaPrivate.openInFlecha(Number(windowId));
      ctx.adoptTab?.(tabId);
    },
  });
}

// ---- split ratio (patch 0019) ------------------------------------------------------------

if (has('setSplitRatio')) {
  // Dragging the gap between the panes: the Store keeps the ratio (store.setSplitRatio clamps it).
  chrome.flechaPrivate.onSplitRatioChanged.addListener(
    when((ctx, tabId: number, ratio: number) => {
      const split = ctx.store.activeSpace.split;
      if (split && ctx.flechaTabId?.(tabId) === split[0]) ctx.store.setSplitRatio(ratio);
    }),
  );
}

/** The active Space's splitRatio on Chromium's split (patch 0019 also draws it flat, 8px apart). */
function registerSplitRatio(ctx: NativeContext) {
  if (!has('setSplitRatio') || !ctx.chromeTabId) return;
  let sent = '';
  const apply = () => {
    const space = ctx.store.activeSpace;
    const left = space.split && ctx.chromeTabId!(space.split[0]);
    if (left === undefined || !space.split) return;
    const ratio = space.splitRatio ?? 0.5;
    const key = `${left}:${ratio}`;
    if (key === sent) return;
    // Fails until Chromium has made the split; tabs.onUpdated (splitViewId) tries again.
    void chrome.flechaPrivate.setSplitRatio(left, ratio).then(
      () => (sent = key),
      () => undefined,
    );
  };
  apply();
  ctx.store.onChange(apply);
  chrome.tabs.onUpdated.addListener((_id, info) => {
    if ('splitViewId' in info) apply();
  });
}

// ---- theme (patch 0018) ------------------------------------------------------------------

/** Settings' Appearance: Chromium's UI and pages' prefers-color-scheme follow state.theme. */
function registerTheme(ctx: NativeContext) {
  if (!has('setTheme')) return;
  let sent: string | undefined;
  const apply = () => {
    const theme = ctx.store.state.theme ?? 'system';
    if (theme === sent) return;
    sent = theme;
    void chrome.flechaPrivate.setTheme(theme);
  };
  apply();
  ctx.store.onChange(apply);
}

// ---- keyboard shortcuts (patch 0016) ----------------------------------------------------

/** Accelerator → command id, as setShortcuts() wants it: the fixed families, then yours. */
export function shortcutMap(state: Store['state']): Record<string, string> {
  const map: Record<string, string> = {};
  for (let i = 1; i <= 9; i++) {
    map[`CmdOrCtrl+${i}`] = `selectTab:${i}`;
    map[`Ctrl+${i}`] = `selectSpace:${i}`;
  }
  // ⌘O in a Flechinha: Open in Flecha, as in the Electron spike (the toolbar's button says so).
  map['CmdOrCtrl+O'] = 'openInFlecha';
  for (const { id } of SHORTCUTS) {
    const keys = keysFor(state, id);
    if (keys) map[keys] = id;
  }
  return map;
}

function registerShortcuts(ctx: NativeContext) {
  if (!has('setShortcuts')) return;
  let sent = '';
  // While Settings records a shortcut the keys go to its page, not to Flecha's commands.
  let recording = false;
  const apply = () => {
    const map = recording ? {} : shortcutMap(ctx.store.state);
    const json = JSON.stringify(map);
    if (json === sent) return;
    sent = json;
    void chrome.flechaPrivate.setShortcuts(map).then((rejected) => {
      if (rejected.length) console.warn('Flecha shell: the browser could not read these shortcuts', rejected);
    });
  };
  apply();
  ctx.store.onChange(apply);
  handle({
    'settings:recording': (on: boolean) => {
      recording = !!on;
      apply();
    },
  });
}
