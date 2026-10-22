import os from 'node:os';
import path from 'node:path';
import { BrowserWindow, app, dialog, ipcMain } from 'electron';
import type { SettingsPatch, SettingsView } from '../shared/types';
import type { Store } from './store';
import { setAdBlocking } from './adblock';
import { SEARCH_ENGINES } from './url';
import { canSetDefaultBrowser, isDefaultBrowser, makeDefaultBrowser } from './defaultBrowser';

/** The ⌘, window: a small page of its own, talking to the main process over `settings:*` IPC. */
export class Settings {
  private win?: BrowserWindow;

  constructor(
    private store: Store,
    private onChanged: () => void,
  ) {
    const fromSettings = (event: Electron.IpcMainInvokeEvent) => !!this.win && event.sender === this.win.webContents;
    ipcMain.handle('settings:get', (event) => (fromSettings(event) ? this.view() : undefined));
    ipcMain.handle('settings:set', (event, patch: SettingsPatch) => (fromSettings(event) ? this.apply(patch) : undefined));
    ipcMain.handle('settings:make-default-browser', (event) => {
      if (!fromSettings(event)) return undefined;
      makeDefaultBrowser();
      return this.view();
    });
    ipcMain.handle('settings:choose-downloads-folder', (event) => (fromSettings(event) ? this.chooseDownloadsFolder() : undefined));
  }

  open() {
    if (this.win && !this.win.isDestroyed()) return this.win.focus();
    this.win = new BrowserWindow({
      width: 560,
      height: 520,
      resizable: false,
      fullscreenable: false,
      title: 'Settings',
      titleBarStyle: 'hiddenInset',
      backgroundColor: '#f5f5f7',
      show: false,
      webPreferences: {
        preload: path.join(__dirname, '../preload/settings-preload.js'),
        sandbox: true,
        contextIsolation: true,
      },
    });
    this.win.loadFile(path.join(__dirname, '../renderer/settings.html'));
    this.win.once('ready-to-show', () => this.win?.show());
  }

  view(): SettingsView {
    return {
      adBlock: this.store.state.adBlock !== false,
      searchEngine: this.store.state.searchEngine ?? 'google',
      downloadsFolder: tildify(this.store.state.downloadsFolder ?? app.getPath('downloads')),
      askWhereToSave: !!this.store.state.askWhereToSave,
      defaultBrowser: { isDefault: isDefaultBrowser(), canSet: canSetDefaultBrowser() },
      searchEngines: Object.entries(SEARCH_ENGINES).map(([id, e]) => ({ id: id as keyof typeof SEARCH_ENGINES, name: e.name })),
    };
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
    // Ignore ids that aren't in the list rather than storing garbage.
    if (patch.searchEngine && patch.searchEngine in SEARCH_ENGINES) {
      this.store.setSearchEngine(patch.searchEngine);
    }
    if (patch.askWhereToSave !== undefined) this.store.setDownloads({ askWhereToSave: patch.askWhereToSave });
    if (patch.adBlock !== undefined) {
      this.store.setAdBlock(patch.adBlock);
      await setAdBlocking(patch.adBlock).catch((err) => console.error('Ad blocker failed', err));
    }
    this.onChanged();
    return this.view();
  }
}

function tildify(folder: string) {
  const home = os.homedir();
  return folder === home || folder.startsWith(home + path.sep) ? '~' + folder.slice(home.length) : folder;
}
