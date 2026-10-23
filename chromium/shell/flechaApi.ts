/**
 * window.flecha for the Chromium build: the same API the Electron preload gives the sidebar,
 * over the shell's port. Imported before the sidebar code so it is there when that runs.
 */
import type { FlechaApi, UiState } from '../../src/shared/types';
import { spaceTint } from '../../src/main/spaceTint';
import { cmd, invoke, on } from './client';

const surface = document.documentElement.dataset.surface;
const systemDark = matchMedia('(prefers-color-scheme: dark)');

/** The background sends the Space's light colour; dark mode darkens it here. */
const stateListeners: Array<(state: UiState) => void> = [];
let lastState: UiState | undefined;
function emitState(state: UiState) {
  lastState = state;
  const dark = state.theme === 'dark' || (state.theme !== 'light' && systemDark.matches);
  const shown = { ...state, tint: spaceTint(state.tint, dark) };
  for (const cb of stateListeners) cb(shown);
}
on('state', emitState);
systemDark.addEventListener('change', () => lastState && emitState(lastState));

const api: FlechaApi = {
  onState: (cb) => void stateListeners.push(cb),
  onPalette: (cb) => on('palette', cb),
  onEditSpace: (cb) => on('edit-space', cb),
  onRenameTab: (cb) => on('rename-tab', cb),
  onPeek: (cb) => on('peek', cb),
  onDownloads: (cb) => on('downloads', cb),
  onTourKey: (cb) => on('tour-key', cb),
  onFlash: (cb) => on('flash', cb),
  ready: cmd('ready'),
  searchHistory: (query) => invoke('searchHistory', query),
  topSites: () => invoke('topSites'),
  // Extensions can't read other apps' icons; the onboarding falls back to the name.
  appIcon: async () => undefined,
  detectBrowsers: () => invoke('detectBrowsers'),
  importBrowser: (id) => invoke('importBrowser', id),
  // On the new tab page, what you type opens in this tab.
  newTab: surface === 'newtab' ? cmd('navigate') : cmd('newTab'),
  navigate: cmd('navigate'),
  activate: cmd('activate'),
  close: cmd('close'),
  back: cmd('back'),
  forward: cmd('forward'),
  reload: cmd('reload'),
  switchSpace: cmd('switchSpace'),
  newSpace: cmd('newSpace'),
  updateSpace: cmd('updateSpace'),
  setSpaceIsolated: cmd('setSpaceIsolated'),
  deleteSpace: cmd('deleteSpace'),
  tabMenu: cmd('tabMenu'),
  spaceMenu: cmd('spaceMenu'),
  toggleFolder: cmd('toggleFolder'),
  renameFolder: cmd('renameFolder'),
  moveTab: cmd('moveTab'),
  clearToday: cmd('clearToday'),
  importMenu: cmd('importMenu'),
  openPalette: cmd('openPalette'),
  closePalette: cmd('closePalette'),
  toggleSidebar: cmd('toggleSidebar'),
  setSidebarPeek: cmd('setSidebarPeek'),
  // The page can write the clipboard itself; the background has no document to do it from.
  copyText: (text) => {
    void navigator.clipboard.writeText(String(text)).then(() => cmd('flash')('Link copied'));
  },
  renameTab: cmd('renameTab'),
  toggleMute: cmd('toggleMute'),
  capturePage: cmd('capturePage'),
  clearSiteData: cmd('clearSiteData'),
  setSitePermission: cmd('setSitePermission'),
  zoom: cmd('zoom'),
  openDevTools: cmd('openDevTools'),
  openSettings: cmd('openSettings'),
  toggleResponsive: cmd('toggleResponsive'),
  showConsole: cmd('showConsole'),
  editBoost: cmd('editBoost'),
  openBoostsFolder: cmd('openBoostsFolder'),
  splitWith: cmd('splitWith'),
  setTabDragging: cmd('setTabDragging'),
  dropToSplit: cmd('dropToSplit'),
  splitNew: cmd('splitNew'),
  closePeek: cmd('closePeek'),
  openDownload: cmd('openDownload'),
  revealDownload: cmd('revealDownload'),
  cancelDownload: cmd('cancelDownload'),
  clearDownloads: cmd('clearDownloads'),
  promotePeek: cmd('promotePeek'),
  finishOnboarding: cmd('finishOnboarding'),
  setPreferences: cmd('setPreferences'),
  makeDefaultBrowser: cmd('makeDefaultBrowser'),
  newFlechinha: cmd('newFlechinha'),
};

window.flecha = api;
