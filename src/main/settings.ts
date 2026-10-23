import os from 'node:os';
import path from 'node:path';
import { BrowserWindow, app, clipboard, dialog, ipcMain, nativeTheme } from 'electron';
import type { SettingsPatch, SettingsView } from '../shared/types';
import type { Store } from './store';
import { setAdBlocking } from './adblock';
import { canSetDefaultBrowser, isDefaultBrowser, makeDefaultBrowser } from './defaultBrowser';
import { applySettings, renewAgentToken, settingsView } from './settingsCore';
import type { TabManager } from './tabManager';
import { electronTabManager, type BrowserTabsHost } from './tabManagerElectron';

/** The ⌘, window: a small page of its own, talking to the main process over `settings:*` IPC. */
export class Settings {
  private win?: BrowserWindow;
  /** Settings is recording a shortcut: keys go to it, not to the page or the menus. */
  private recording = false;
  /** Settings → Tabs & Spaces. */
  readonly tabs: TabManager;

  constructor(
    private store: Store,
    private onChanged: () => void,
    private agent: { setEnabled(enabled: boolean): Promise<void>; endpoint(): string | undefined },
    openBoosts: () => void,
    showTour: () => void,
    tabs: BrowserTabsHost,
  ) {
    this.tabs = electronTabManager(store, tabs, () => this.win);
    const fromSettings = (event: Electron.IpcMainInvokeEvent) => !!this.win && !this.win.isDestroyed() && event.sender === this.win.webContents;
    ipcMain.handle('settings:get', (event) => (fromSettings(event) ? this.view() : undefined));
    ipcMain.handle('settings:set', (event, patch: SettingsPatch) => (fromSettings(event) ? this.apply(patch) : undefined));
    ipcMain.handle('settings:new-agent-token', (event) => {
      if (!fromSettings(event)) return undefined;
      renewAgentToken(this.store);
      return this.view();
    });
    ipcMain.handle('settings:open-boosts', (event) => {
      if (fromSettings(event)) openBoosts();
    });
    ipcMain.handle('settings:copy', (event, text: string) => {
      if (fromSettings(event)) clipboard.writeText(String(text));
    });
    ipcMain.handle('settings:make-default-browser', (event) => {
      if (!fromSettings(event)) return undefined;
      makeDefaultBrowser();
      return this.view();
    });
    ipcMain.handle('settings:show-tour', (event) => {
      if (!fromSettings(event)) return;
      // The tour plays in the browser window; Settings steps out of its way.
      this.win?.close();
      showTour();
    });
    ipcMain.handle('settings:record-shortcut', (event, recording: boolean) => {
      if (fromSettings(event)) this.recording = !!recording;
    });
    ipcMain.handle('settings:choose-downloads-folder', (event) => (fromSettings(event) ? this.chooseDownloadsFolder() : undefined));
  }

  /** ⌘F goes to Settings' search when Settings is the window in front. Returns whether it did. */
  focusSearch() {
    if (!this.win || this.win.isDestroyed() || !this.win.isFocused()) return false;
    this.win.webContents.send('settings:find');
    return true;
  }

  /** Opens Settings, optionally on a given pane (e.g. `shortcuts` from the Help menu). */
  open(pane?: string) {
    if (this.win && !this.win.isDestroyed()) {
      if (pane) this.win.webContents.send('settings:pane', pane);
      return this.win.focus();
    }
    this.win = new BrowserWindow({
      width: 780,
      height: 580,
      // Taller and wider for long lists of tabs in Tabs & Spaces; never smaller than the panes need.
      minWidth: 780,
      minHeight: 580,
      fullscreenable: false,
      minimizable: false,
      title: 'Settings',
      titleBarStyle: 'hiddenInset',
      trafficLightPosition: { x: 18, y: 18 },
      // The sidebar shows the desktop through macOS's translucent material, like System Settings.
      vibrancy: 'sidebar',
      visualEffectState: 'followWindow',
      backgroundColor: '#00000000',
      show: false,
      webPreferences: {
        preload: path.join(__dirname, '../preload/settings-preload.js'),
        sandbox: true,
        contextIsolation: true,
      },
    });
    this.win.loadFile(path.join(__dirname, '../renderer/settings.html'), pane ? { hash: pane } : undefined);
    // Runs before the menus see the key, so recording ⌘T doesn't also open a tab.
    this.win.webContents.on('before-input-event', (event, input) => {
      if (!this.recording || input.type !== 'keyDown') return;
      event.preventDefault();
      const { code, key, shift, control, alt, meta } = input;
      this.win?.webContents.send('settings:key', { code, key, shift, control, alt, meta });
    });
    this.win.on('blur', () => (this.recording = false));
    this.win.once('ready-to-show', () => this.win?.show());
  }

  view(): SettingsView {
    return settingsView(this.store, {
      downloadsFolder: tildify(this.store.state.downloadsFolder ?? app.getPath('downloads')),
      defaultBrowser: { isDefault: isDefaultBrowser(), canSet: canSetDefaultBrowser() },
      agentEndpoint: this.agent.endpoint(),
    });
  }

  private async chooseDownloadsFolder(): Promise<SettingsView> {
    const { canceled, filePaths } = await dialog.showOpenDialog(this.win!, {
      title: 'Choose where downloads are saved',
      defaultPath: this.store.state.downloadsFolder ?? app.getPath('downloads'),
      properties: ['openDirectory', 'createDirectory'],
    });
    if (!canceled && filePaths[0]) this.store.setDownloads({ folder: filePaths[0] });
    return this.view();
  }

  private async apply(patch: SettingsPatch): Promise<SettingsView> {
    const notice = await applySettings(this.store, patch, {
      theme: (theme) => (nativeTheme.themeSource = theme),
      agent: (enabled) => this.agent.setEnabled(enabled),
      adBlock: (enabled) => setAdBlocking(enabled),
    });
    this.onChanged();
    return { ...this.view(), notice };
  }
}

function tildify(folder: string) {
  const home = os.homedir();
  return folder === home || folder.startsWith(home + path.sep) ? '~' + folder.slice(home.length) : folder;
}
