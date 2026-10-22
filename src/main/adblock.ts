import fs from 'node:fs/promises';
import path from 'node:path';
import { app, ipcMain, session, type Session } from 'electron';
import { ElectronBlocker } from '@ghostery/adblocker-electron';

const REFRESH_AFTER_MS = 3 * 24 * 60 * 60 * 1000;

let blocker: ElectronBlocker | undefined;
let enabled = false;
const sessions = new Set<Session>();

/**
 * Built-in ad and tracker blocking, using the same EasyList / EasyPrivacy / uBlock lists.
 *
 * This lives in the browser rather than in an extension because Electron exposes
 * chrome.declarativeNetRequest but never applies its rules, so uBlock Origin Lite installs
 * and matches ads yet blocks nothing (and MV2 uBlock Origin is gone from the Web Store).
 */
export async function setAdBlocking(on: boolean) {
  enabled = on;
  sessions.add(session.defaultSession);
  if (on) blocker ??= await loadBlocker();
  for (const ses of sessions) apply(ses);
}

/** Space sessions (separate cookies) follow the same on/off setting. */
export function addAdBlockSession(ses: Session) {
  sessions.add(ses);
  apply(ses);
}

function apply(ses: Session) {
  if (!blocker) return;
  const blocking = blocker.isBlockingEnabled(ses);
  if (enabled && !blocking) enableIn(blocker, ses);
  else if (!enabled && blocking) blocker.disableBlockingInSession(ses);
}

/**
 * The library registers its cosmetic-filter IPC handlers globally every time it is enabled,
 * which throws for a second session. The first registration already serves every session,
 * so later ones skip it.
 */
function enableIn(engine: ElectronBlocker, ses: Session) {
  const handle = ipcMain.handle;
  ipcMain.handle = ((channel, listener) => {
    try {
      handle.call(ipcMain, channel, listener);
    } catch {
      // Already registered by the first session.
    }
  }) as typeof ipcMain.handle;
  try {
    engine.enableBlockingInSession(ses);
  } finally {
    ipcMain.handle = handle;
  }
}

async function loadBlocker() {
  const cache = path.join(app.getPath('userData'), 'adblock-engine.bin');
  // The engine is cached on disk; drop it now and then so the filter lists stay fresh.
  const stat = await fs.stat(cache).catch(() => undefined);
  if (stat && Date.now() - stat.mtimeMs > REFRESH_AFTER_MS) await fs.rm(cache, { force: true });
  return ElectronBlocker.fromPrebuiltAdsAndTracking(fetch, { path: cache, read: fs.readFile, write: fs.writeFile });
}
