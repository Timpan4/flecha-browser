import os from 'node:os';
import path from 'node:path';
import { randomBytes } from 'node:crypto';
import { BrowserWindow, app, clipboard, dialog, ipcMain, nativeTheme } from 'electron';
import type { SettingsPatch, SettingsView } from '../shared/types';
import type { Store } from './store';
import { setAdBlocking } from './adblock';
import { SEARCH_ENGINES } from './url';
import { sanitize as sanitizeEnvironments } from './environments';
import { canSetDefaultBrowser, isDefaultBrowser, makeDefaultBrowser } from './defaultBrowser';

/** The ⌘, window: a small page of its own, talking to the main process over `settings:*` IPC. */
export class Settings {
  private win?: BrowserWindow;

  constructor(
    private store: Store,
    private onChanged: () => void,
    private agent: { setEnabled(enabled: boolean): Promise<void>; endpoint(): string | undefined },
    openBoosts: () => void,
  ) {
    const fromSettings = (event: Electron.IpcMainInvokeEvent) => !!this.win && event.sender === this.win.webContents;
    ipcMain.handle('settings:get', (event) => (fromSettings(event) ? this.view() : undefined));
    ipcMain.handle('settings:set', (event, patch: SettingsPatch) => (fromSettings(event) ? this.apply(patch) : undefined));
    ipcMain.handle('settings:new-agent-token', (event) => {
      if (!fromSettings(event)) return undefined;
      // The bridge reads the token on every request, so the old one stops working right away.
      this.store.setAgent({ enabled: !!this.store.state.agent?.enabled, token: newToken() });
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
    ipcMain.handle('settings:choose-downloads-folder', (event) => (fromSettings(event) ? this.chooseDownloadsFolder() : undefined));
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
      resizable: false,
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
    this.win.once('ready-to-show', () => this.win?.show());
  }

  view(): SettingsView {
    return {
      adBlock: this.store.state.adBlock !== false,
      searchEngine: this.store.state.searchEngine ?? 'google',
      downloadsFolder: tildify(this.store.state.downloadsFolder ?? app.getPath('downloads')),
      askWhereToSave: !!this.store.state.askWhereToSave,
      defaultBrowser: { isDefault: isDefaultBrowser(), canSet: canSetDefaultBrowser() },
      theme: this.store.state.theme ?? 'system',
      archiveAfterHours: this.store.state.archiveAfterHours ?? 12,
      peekPinnedLinks: this.store.state.peekPinnedLinks !== false,
      openLinksInFlechinha: this.store.state.openLinksInFlechinha !== false,
      agent: {
        enabled: !!this.store.state.agent?.enabled,
        endpoint: this.agent.endpoint(),
        token: this.store.state.agent?.enabled ? this.store.state.agent.token : undefined,
      },
      environments: (this.store.state.environments ?? []).map((e) => ({
        id: e.id,
        name: e.name,
        targetsText: e.targets.map((t) => `${t.label} ${t.origin}`).join('\n'),
      })),
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
    if (patch.environments !== undefined) this.store.setEnvironments(sanitizeEnvironments(patch.environments));
    if (patch.askWhereToSave !== undefined) this.store.setDownloads({ askWhereToSave: patch.askWhereToSave });
    if (patch.theme || patch.peekPinnedLinks !== undefined) {
      this.store.setPreferences({ theme: patch.theme, peekPinnedLinks: patch.peekPinnedLinks });
      nativeTheme.themeSource = this.store.state.theme ?? 'system';
    }
    if (patch.openLinksInFlechinha !== undefined) this.store.setOpenLinksInFlechinha(patch.openLinksInFlechinha);
    if (typeof patch.archiveAfterHours === 'number') this.store.setArchiveAfter(patch.archiveAfterHours);
    if (patch.agentEnabled !== undefined) {
      this.store.setAgent({ enabled: patch.agentEnabled, token: this.store.state.agent?.token || newToken() });
      await this.agent.setEnabled(patch.agentEnabled).catch((err) => console.error('Agent bridge failed', err));
    }
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

function newToken() {
  return randomBytes(24).toString('base64url');
}
