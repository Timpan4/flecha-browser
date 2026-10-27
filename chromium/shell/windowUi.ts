/**
 * What the Electron spike drew natively over the sidebar and an extension page draws itself:
 * the tab and Space context menus, the "Delete Space?" question, and clipboard writes the
 * background asks for. The background builds menus as data (protocol.ts MenuRun); this page
 * shows them with settingsUi's in-page menu.
 *
 * In Flecha's own browser the frame page is only as wide as the sidebar, so while a menu or
 * a question is up it covers the whole window (an "overlay", like the command bar): clicks
 * on the page dismiss the menu, and the dialog has room.
 */
import type { DetectedBrowser, ImportResult, UiState } from '../shared/types';
import { cmd, invoke, on } from './client';
import type { MenuRun } from './protocol';
import { askDialog, popupMenu, type MenuEntry, type MenuOptions } from './settingsUi';
import { DELETE_SPACE_DETAIL } from './lib/undo';

const framed = typeof chrome.flechaPrivate?.setFrame === 'function';
const surface = document.documentElement.dataset.surface;
const windowId = framed && surface === 'window' ? chrome.flechaPrivate.getWindowId() : undefined;

let overlaid = false;
on('frame', (state: { overlay: boolean }) => (overlaid = state.overlay));

/** Resolves on the next resize (the frame growing), or after `ms` if none comes. */
const resized = (ms: number) =>
  new Promise<void>((resolve) => {
    const done = () => {
      clearTimeout(timer);
      removeEventListener('resize', done);
      resolve();
    };
    const timer = setTimeout(done, ms);
    addEventListener('resize', done);
  });

/** Covers the window with the frame while `fn` runs. */
async function overWindow<T>(name: string, fn: () => Promise<T>): Promise<T> {
  if (!windowId) return fn();
  const grow = !overlaid;
  cmd('setOverlay')(name, true);
  if (grow) await resized(300);
  try {
    return await fn();
  } finally {
    cmd('setOverlay')(name, false);
  }
}

export interface WindowUiHooks {
  /** Starts the sidebar's inline rename of a tab. */
  rename(tabId: string): void;
  /** Starts the sidebar's inline edit of a pinned tab's or favorite's URL. */
  editHomeUrl(tabId: string): void;
  state(): UiState | undefined;
}

let hooks: WindowUiHooks | undefined;
export function setupWindowUi(h: WindowUiHooks) {
  hooks = h;
}

/** Asks for the background's menu `name` about `id`, shows it at the pointer and runs the pick. */
export async function showMenu(name: 'tabMenu' | 'spaceMenu', id: string) {
  const items = await invoke<MenuEntry<MenuRun>[] | undefined>(name, id);
  if (!items?.length) return;
  const picked = await overWindow('menu', () => popupMenu(items));
  if (picked) await runMenuItem(picked.run);
}

/** A menu of the page's own (the window menu on Linux and Windows), over the window. */
export function showPageMenu<T>(items: MenuEntry<T>[], options: MenuOptions) {
  return overWindow('menu', () => popupMenu(items, options));
}

async function runMenuItem(run: MenuRun) {
  if ('cmd' in run) cmd(run.cmd)(...run.args);
  else if ('copy' in run) await copy(run.copy, run.message);
  else if ('rename' in run || 'editHomeUrl' in run) {
    // The keys go to the sidebar's field, so the frame page takes keyboard focus.
    if (windowId) await chrome.flechaPrivate.focus('frame', await windowId);
    if ('rename' in run) hooks?.rename(run.rename);
    else hooks?.editHomeUrl(run.editHomeUrl);
  }
  else if ('deleteSpace' in run) await confirmDeleteSpace(run.deleteSpace);
  else if ('clearSpaceData' in run) await confirmClearSpaceData(run.clearSpaceData);
}

/**
 * Import from another browser, from the window menu or the command bar: one of the browsers the
 * import helper finds (as the onboarding tour lists them), then what came over.
 */
export async function importFromBrowser() {
  await overWindow('dialog', async () => {
    const note = (message: string, detail: string) => askDialog({ message, detail, buttons: ['OK'], defaultId: 0, cancelId: 0 });
    let browsers: DetectedBrowser[];
    try {
      browsers = await invoke<DetectedBrowser[]>('detectBrowsers');
    } catch (error) {
      return note('Couldn’t look for your browsers', error instanceof Error ? error.message : 'Browser discovery failed.');
    }
    if (!browsers.length) {
      return note('No browsers to import from', 'No supported browser data found in the standard profile folders on this computer. Custom locations and sandboxed installations may not be detected.');
    }
    const picked = browsers[await askDialog({
      message: 'Import from which browser?',
      detail: 'Its bookmarks and history come over to Flecha.',
      buttons: [...browsers.map((b) => b.name), 'Cancel'],
      defaultId: 0,
      cancelId: browsers.length,
    })];
    if (!picked) return;
    try {
      const r = await invoke<ImportResult>('importBrowser', picked.id);
      const parts = [r.bookmarks && `${r.bookmarks.toLocaleString()} bookmark${r.bookmarks === 1 ? '' : 's'}`, r.history && `${r.history.toLocaleString()} page${r.history === 1 ? '' : 's'}`].filter(Boolean);
      return note(`Imported from ${picked.name}`, [...parts, ...r.warnings].join(' · ') || 'Nothing to import');
    } catch {
      return note(`Couldn’t import ${picked.name}`, 'Try again.');
    }
  });
}

/** Electron's "Sign out of everything in “Space”?" message box, in the page. */
async function confirmClearSpaceData(spaceId: string) {
  const space = hooks?.state()?.spaces.find((s) => s.id === spaceId);
  if (!space?.isolated) return;
  const response = await overWindow('dialog', () =>
    askDialog({
      message: `Sign out of everything in “${space.name}”?`,
      detail: 'Clears cookies, storage and cache for this Space only.',
      buttons: ['Clear', 'Cancel'],
      defaultId: 1,
      cancelId: 1,
    }),
  );
  if (response === 0) cmd('clearSpaceData')(spaceId);
}

/** Electron's "Delete “Space”?" message box, in the page. */
export async function confirmDeleteSpace(spaceId: string) {
  const state = hooks?.state();
  const space = state?.spaces.find((s) => s.id === spaceId);
  if (!space || state!.spaces.length < 2) return;
  const response = await overWindow('dialog', () =>
    askDialog({
      message: `Delete “${space.name}”?`,
      detail: DELETE_SPACE_DETAIL,
      buttons: ['Delete Space', 'Cancel'],
      defaultId: 1,
      cancelId: 1,
    }),
  );
  if (response === 0) cmd('deleteSpace')(spaceId);
}

/**
 * Writes the clipboard. A keyboard shortcut leaves the page focused, not this one, and the
 * clipboard only takes writes from the focused document, so the frame borrows focus for it.
 */
async function writeClipboard(item: string | Blob) {
  const write = () =>
    typeof item === 'string' ? navigator.clipboard.writeText(item) : navigator.clipboard.write([new ClipboardItem({ [item.type]: item })]);
  if (document.hasFocus() || !windowId) return write();
  const id = await windowId;
  await chrome.flechaPrivate.focus('frame', id);
  try {
    await write();
  } finally {
    await chrome.flechaPrivate.focus('page', id);
  }
}

/** A data: URL as a Blob, without fetch (the page's CSP keeps connect-src to 'self'). */
function pngBlob(dataUrl: string) {
  const bytes = Uint8Array.from(atob(dataUrl.slice(dataUrl.indexOf(',') + 1)), (c) => c.charCodeAt(0));
  return new Blob([bytes], { type: 'image/png' });
}

export async function copy(text: string, message: string) {
  try {
    await writeClipboard(text);
    cmd('flash')(message);
  } catch (err) {
    console.error('Flecha shell: copying failed', err);
  }
}

// The background's clipboard requests (copy link, screenshot from a shortcut) go to the frame
// page of the window they're for; stock Chromium's side panel takes them otherwise.
if (surface === 'window' || (surface === 'panel' && !framed)) {
  on('clipboard', async (target: number | undefined, data: { text?: string; png?: string; message: string }) => {
    if (windowId && target !== undefined && target !== (await windowId)) return;
    try {
      if (data.png) await writeClipboard(pngBlob(data.png));
      else if (data.text !== undefined) await writeClipboard(data.text);
      cmd('flash')(data.message);
    } catch (err) {
      console.error('Flecha shell: copying failed', err);
    }
  });
}
