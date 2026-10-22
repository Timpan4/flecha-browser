import { contextBridge, ipcRenderer } from 'electron';
import { injectBrowserAction } from 'electron-chrome-extensions/browser-action';
import type { FlechaApi } from '../shared/types';

const cmd =
  (name: string) =>
  (...args: unknown[]) =>
    ipcRenderer.send('cmd', name, ...args);

const api: FlechaApi = {
  onState: (cb) => void ipcRenderer.on('state', (_e, state) => cb(state)),
  onPalette: (cb) => void ipcRenderer.on('palette', (_e, mode) => cb(mode)),
  onRenameSpace: (cb) => void ipcRenderer.on('rename-space', (_e, id) => cb(id)),
  onPeek: (cb) => void ipcRenderer.on('peek', (_e, peek) => cb(peek)),
  ready: cmd('ready'),
  searchHistory: (query) => ipcRenderer.invoke('search-history', query),
  newTab: cmd('newTab'),
  navigate: cmd('navigate'),
  activate: cmd('activate'),
  close: cmd('close'),
  back: cmd('back'),
  forward: cmd('forward'),
  reload: cmd('reload'),
  switchSpace: cmd('switchSpace'),
  newSpace: cmd('newSpace'),
  renameSpace: cmd('renameSpace'),
  tabMenu: cmd('tabMenu'),
  spaceMenu: cmd('spaceMenu'),
  toggleFolder: cmd('toggleFolder'),
  renameFolder: cmd('renameFolder'),
  moveTab: cmd('moveTab'),
  clearToday: cmd('clearToday'),
  importArc: cmd('importArc'),
  openPalette: cmd('openPalette'),
  closePalette: cmd('closePalette'),
  toggleSidebar: cmd('toggleSidebar'),
  splitWith: cmd('splitWith'),
  splitNew: cmd('splitNew'),
  closePeek: cmd('closePeek'),
  promotePeek: cmd('promotePeek'),
};

contextBridge.exposeInMainWorld('flecha', api);
// Defines the <browser-action-list> element that shows extension buttons in the sidebar.
injectBrowserAction();
