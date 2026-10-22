import { dialog, type BaseWindow, type Session, type WebContents } from 'electron';
import type { Store } from './store';

/** Granted without asking: harmless, and asking would be noise. */
const SILENT = new Set(['fullscreen', 'clipboard-sanitized-write', 'pointerLock']);

/** What a site is asking for, in the words of the prompt. */
const ASKED: Record<string, string> = {
  geolocation: 'know your location',
  media: 'use your camera and microphone',
  notifications: 'send you notifications',
  'display-capture': 'share your screen',
  midi: 'use your MIDI devices',
  'clipboard-read': 'read your clipboard',
};

export const PERMISSION_LABELS: Record<string, string> = {
  geolocation: 'Location',
  media: 'Camera & microphone',
  notifications: 'Notifications',
  'display-capture': 'Screen sharing',
  midi: 'MIDI devices',
  'clipboard-read': 'Clipboard',
};

/**
 * Site permissions: asked once per site, remembered in the browser state, and reported
 * back to pages that check before asking (otherwise they'd keep asking on every visit).
 */
export function registerPermissions(
  ses: Session,
  store: Store,
  /** The window a page's prompt is attached to. */
  windowFor: (wc: WebContents) => BaseWindow,
  isUi: (wc: WebContents) => boolean,
) {
  const pending = new Map<string, Promise<boolean>>();

  ses.setPermissionCheckHandler((wc, permission, requestingOrigin) => {
    if (SILENT.has(permission)) return true;
    if (wc && isUi(wc)) return false;
    return store.sitePermission(originOf(requestingOrigin), permission) === 'allow';
  });

  ses.setPermissionRequestHandler(async (wc, permission, callback, details) => {
    if (isUi(wc) || !(permission in ASKED)) return callback(SILENT.has(permission));
    const origin = originOf(details.requestingUrl);
    const saved = store.sitePermission(origin, permission);
    if (saved) return callback(saved === 'allow');

    // Pages often request the same thing several times at once; ask the user only once.
    const key = `${origin} ${permission}`;
    let answer = pending.get(key);
    if (!answer) {
      answer = ask(windowFor(wc), origin, permission).then(({ allow, remember }) => {
        if (remember) store.setSitePermission(origin, permission, allow ? 'allow' : 'block');
        return allow;
      });
      pending.set(key, answer);
      void answer.finally(() => pending.delete(key));
    }
    callback(await answer);
  });
}

async function ask(win: BaseWindow, origin: string, permission: string) {
  const { response, checkboxChecked } = await dialog.showMessageBox(win, {
    message: `${new URL(origin).host} wants to ${ASKED[permission]}`,
    buttons: ['Allow', 'Block'],
    defaultId: 0,
    cancelId: 1,
    checkboxLabel: 'Remember for this site',
    checkboxChecked: true,
  });
  return { allow: response === 0, remember: checkboxChecked };
}

export function originOf(url: string) {
  try {
    return new URL(url).origin;
  } catch {
    return url;
  }
}
