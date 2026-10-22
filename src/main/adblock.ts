import fs from 'node:fs/promises';
import path from 'node:path';
import { app, session } from 'electron';
import { ElectronBlocker } from '@ghostery/adblocker-electron';

const REFRESH_AFTER_MS = 3 * 24 * 60 * 60 * 1000;

let blocker: ElectronBlocker | undefined;

/**
 * Built-in ad and tracker blocking, using the same EasyList / EasyPrivacy / uBlock lists.
 *
 * This lives in the browser rather than in an extension because Electron exposes
 * chrome.declarativeNetRequest but never applies its rules, so uBlock Origin Lite installs
 * and matches ads yet blocks nothing (and MV2 uBlock Origin is gone from the Web Store).
 */
export async function setAdBlocking(enabled: boolean) {
  const ses = session.defaultSession;
  if (!enabled) {
    if (blocker?.isBlockingEnabled(ses)) blocker.disableBlockingInSession(ses);
    return;
  }
  blocker ??= await loadBlocker();
  if (!blocker.isBlockingEnabled(ses)) blocker.enableBlockingInSession(ses);
}

async function loadBlocker() {
  const cache = path.join(app.getPath('userData'), 'adblock-engine.bin');
  // The engine is cached on disk; drop it now and then so the filter lists stay fresh.
  const stat = await fs.stat(cache).catch(() => undefined);
  if (stat && Date.now() - stat.mtimeMs > REFRESH_AFTER_MS) await fs.rm(cache, { force: true });
  return ElectronBlocker.fromPrebuiltAdsAndTracking(fetch, { path: cache, read: fs.readFile, write: fs.writeFile });
}
