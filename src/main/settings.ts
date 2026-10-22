import path from 'node:path';
import { BrowserWindow, ipcMain } from 'electron';
import type { SettingsPatch, SettingsView } from '../shared/types';
import type { Store } from './store';
import { setAdBlocking } from './adblock';

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
    return { adBlock: this.store.state.adBlock !== false };
  }

  private async apply(patch: SettingsPatch): Promise<SettingsView> {
    if (patch.adBlock !== undefined) {
      this.store.setAdBlock(patch.adBlock);
      await setAdBlocking(patch.adBlock).catch((err) => console.error('Ad blocker failed', err));
    }
    this.onChanged();
    return this.view();
  }
}
