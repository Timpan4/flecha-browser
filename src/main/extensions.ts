import path from 'node:path';
import { BrowserWindow, dialog, session, type BaseWindow, type WebContents } from 'electron';
import { ElectronChromeExtensions } from 'electron-chrome-extensions';
import { installChromeWebStore, uninstallExtension } from 'electron-chrome-web-store';

export const WEB_STORE_URL = 'https://chromewebstore.google.com/';

/** What the extension system needs from the browser to answer chrome.tabs calls. */
export interface TabHost {
  readonly win: BaseWindow;
  createTabFor(url?: string): WebContents;
  selectTabFor(wc: WebContents): void;
  removeTabFor(wc: WebContents): void;
}

/**
 * Chrome extension support: electron-chrome-extensions fills in the chrome.* APIs Electron
 * lacks (tabs, action popups, context menus), and electron-chrome-web-store makes
 * "Add to Chrome" work on the Chrome Web Store and keeps installed extensions updated.
 *
 * Must run before any tab is created so the extension preload reaches every page.
 */
export async function setupExtensions(getHost: () => TabHost | undefined) {
  const ses = session.defaultSession;
  // The library calls back while the first tabs are being created, before the browser exists.
  const host = () => {
    const h = getHost();
    if (!h) throw new Error('Browser is not ready yet');
    return h;
  };

  const extensions = new ElectronChromeExtensions({
    // The library is dual licensed; GPL-3.0 means Flecha must be GPL if distributed.
    license: 'GPL-3.0',
    session: ses,
    createTab: async (details) => [host().createTabFor(details.url), host().win],
    selectTab: (wc) => getHost()?.selectTabFor(wc),
    removeTab: (wc) => getHost()?.removeTabFor(wc),
    // Extensions such as password managers open small popup windows for sign-in and unlock.
    createWindow: async (details) => {
      const win = new BrowserWindow({
        width: details.width ?? 420,
        height: details.height ?? 640,
        webPreferences: { session: ses, sandbox: true, contextIsolation: true },
      });
      const url = Array.isArray(details.url) ? details.url[0] : details.url;
      if (url) win.loadURL(url);
      extensions.addTab(win.webContents, win);
      return win;
    },
  });
  ElectronChromeExtensions.handleCRXProtocol(ses);
  const compat = path.join(__dirname, '../preload/extension-compat.js');
  ses.registerPreloadScript({ id: 'flecha-browser-namespace-sw', type: 'service-worker', filePath: compat });
  ses.registerPreloadScript({ id: 'flecha-browser-namespace-frame', type: 'frame', filePath: compat });

  await installChromeWebStore({
    session: ses,
    // Allow MV2 too, for extensions such as the original uBlock Origin.
    minimumManifestVersion: 2,
    beforeInstall: async (details) => {
      const permissions = [...(details.manifest.permissions ?? []), ...(details.manifest.host_permissions ?? [])];
      const { response } = await dialog.showMessageBox(host().win, {
        message: `Add “${details.localizedName}” to Flecha?`,
        detail: permissions.length ? `It will be able to use: ${permissions.join(', ')}` : undefined,
        icon: details.icon,
        buttons: ['Add Extension', 'Cancel'],
        defaultId: 0,
        cancelId: 1,
      });
      return { action: response === 0 ? 'allow' : 'deny' };
    },
  });

  return extensions;
}

export function installedExtensions() {
  return session.defaultSession.extensions.getAllExtensions();
}

export async function removeExtension(win: BaseWindow, id: string, name: string) {
  const { response } = await dialog.showMessageBox(win, {
    message: `Remove “${name}”?`,
    buttons: ['Remove', 'Cancel'],
    defaultId: 1,
    cancelId: 1,
  });
  if (response === 0) await uninstallExtension(id, { session: session.defaultSession });
}
