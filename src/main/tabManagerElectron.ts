import { BrowserWindow, Menu, app, dialog, ipcMain, type MenuItemConstructorOptions, type WebContents } from 'electron';
import type { TabsEdit, TabsMenuAction } from '../shared/types';
import type { Store } from './store';
import { sessionForSpace } from './sessions';
import { TabManager, type AskOptions, type TabsMenuItem, type TabsMenuRequest } from './tabManager';

/** What Settings → Tabs & Spaces needs from Electron's browser window: its tab pages. */
export interface BrowserTabsHost {
  /** The tab's page, only if it is loaded. */
  loadedContents(tabId: string): WebContents | undefined;
  destroyView(tabId: string): void;
  /** Activates the tab and brings the browser window forward. */
  show(tabId: string): void;
  faviconHosts(): string[];
}

/** The tab manager in Electron: `settings:*` IPC from the Settings window, native menus and dialogs. */
export function electronTabManager(store: Store, browser: BrowserTabsHost, win: () => BrowserWindow | undefined): TabManager {
  const settingsWindow = () => {
    const w = win();
    return w && !w.isDestroyed() ? w : undefined;
  };
  const tabs = new TabManager(store, {
    isOpen: (tabId) => !!browser.loadedContents(tabId),
    load: (tabId, url) => void browser.loadedContents(tabId)?.loadURL(url),
    destroyView: (tabId) => browser.destroyView(tabId),
    show: (tabId) => browser.show(tabId),
    faviconHosts: () => browser.faviconHosts(),
    sessionOf: (space) => sessionForSpace(space),
    ask: async (options: AskOptions) => {
      const w = settingsWindow();
      return (await (w ? dialog.showMessageBox(w, options) : dialog.showMessageBox(options))).response;
    },
    push: (view) => settingsWindow()?.webContents.send('settings:tabs', view),
  });

  const fromSettings = (event: Electron.IpcMainInvokeEvent) => event.sender === settingsWindow()?.webContents;
  ipcMain.handle('settings:tabs', (event) => (fromSettings(event) ? tabs.view() : undefined));
  ipcMain.handle('settings:edit-tabs', (event, edit: TabsEdit) => (fromSettings(event) ? tabs.edit(edit) : undefined));
  ipcMain.handle('settings:tabs-menu', (event, request: TabsMenuRequest) => (fromSettings(event) ? popup(tabs, request, settingsWindow()) : undefined));
  ipcMain.handle('settings:show-tab', (event, tabId: string) => {
    if (fromSettings(event)) tabs.show(tabId);
  });
  ipcMain.handle('settings:emoji-panel', (event) => {
    if (fromSettings(event)) app.showEmojiPanel();
  });
  return tabs;
}

/** The native menu for a selection of tabs or one folder. Resolves with what the page should do, if anything. */
function popup(tabs: TabManager, request: TabsMenuRequest, w: BrowserWindow | undefined): Promise<TabsMenuAction | undefined> {
  const items = tabs.menu(request);
  if (!w || !items.length) return Promise.resolve(undefined);
  return new Promise((resolve) => {
    const native = (item: TabsMenuItem): MenuItemConstructorOptions =>
      'separator' in item
        ? { type: 'separator' }
        : 'submenu' in item
          ? { label: item.label, submenu: item.submenu.map(native) }
          : {
              label: item.label,
              click: () => {
                const answer = tabs.run(item.run);
                if (answer) resolve(answer);
              },
            };
    // Clicks can arrive just after the menu closes, so give them a moment before answering "nothing".
    Menu.buildFromTemplate(items.map(native)).popup({ window: w, callback: () => setTimeout(() => resolve(undefined), 50) });
  });
}
