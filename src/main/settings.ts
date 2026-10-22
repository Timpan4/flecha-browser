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
import { SHORTCUTS, displayKeys, isShortcutId, keysFor, sameKeys, shortcutList } from './shortcuts';

/** A modifier and a key, or a function key alone: "Alt+Cmd+K", "F5". */
const ACCELERATOR = /^((Ctrl|Alt|Shift|Cmd)\+){1,4}[^+]+$|^F\d{1,2}$/;

/** The ⌘, window: a small page of its own, talking to the main process over `settings:*` IPC. */
export class Settings {
  private win?: BrowserWindow;
  /** Settings is recording a shortcut: keys go to it, not to the page or the menus. */
  private recording = false;

  constructor(
    private store: Store,
    private onChanged: () => void,
    private agent: { setEnabled(enabled: boolean): Promise<void>; endpoint(): string | undefined },
    openBoosts: () => void,
    showTour: () => void,
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
    return {
      adBlock: this.store.state.adBlock !== false,
      searchEngine: this.store.state.searchEngine ?? 'google',
      downloadsFolder: tildify(this.store.state.downloadsFolder ?? app.getPath('downloads')),
      askWhereToSave: !!this.store.state.askWhereToSave,
      defaultBrowser: { isDefault: isDefaultBrowser(), canSet: canSetDefaultBrowser() },
      theme: this.store.state.theme ?? 'system',
      archiveAfterHours: this.store.state.archiveAfterHours ?? 12,
      peekPinnedLinks: this.store.state.peekPinnedLinks !== false,
      shortcuts: shortcutList(this.store.state),
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

  /** Records one shortcut. A command that had the same keys loses them, and the view says so. */
  private setShortcut(id: string, keys: string | null): string | undefined {
    if (!isShortcutId(id)) return undefined;
    if (keys === 'default') {
      this.store.setKeybinding(id, undefined);
      return undefined;
    }
    if (keys !== null && !ACCELERATOR.test(keys)) return undefined;
    // ⌘1…⌘9 and ⌃1…⌃9 pick tabs and Spaces.
    if (keys && /^(Cmd|Ctrl)\+\d$/.test(keys)) return `${displayKeys(keys)} already goes to a tab or Space.`;
    let notice: string | undefined;
    if (keys) {
      for (const other of SHORTCUTS) {
        const otherKeys = keysFor(this.store.state, other.id);
        if (other.id !== id && otherKeys && sameKeys(otherKeys, keys)) {
          this.store.setKeybinding(other.id, null);
          notice = `${displayKeys(keys)} was “${other.label}”, which now has no shortcut.`;
        }
      }
    }
    const fallback = SHORTCUTS.find((s) => s.id === id)!.keys;
    this.store.setKeybinding(id, keys && sameKeys(keys, fallback) ? undefined : keys);
    return notice;
  }

  private async apply(patch: SettingsPatch): Promise<SettingsView> {
    let notice: string | undefined;
    if (patch.resetShortcuts) this.store.resetKeybindings();
    if (patch.shortcut) notice = this.setShortcut(String(patch.shortcut.id), patch.shortcut.keys === null ? null : String(patch.shortcut.keys));
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
    return { ...this.view(), notice };
  }
}

function tildify(folder: string) {
  const home = os.homedir();
  return folder === home || folder.startsWith(home + path.sep) ? '~' + folder.slice(home.length) : folder;
}

function newToken() {
  return randomBytes(24).toString('base64url');
}
