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
  resolveCtx(ctx);
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
