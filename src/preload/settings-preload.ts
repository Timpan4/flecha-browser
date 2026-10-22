import { contextBridge, ipcRenderer } from 'electron';
import type { SettingsApi } from '../shared/types';

const api: SettingsApi = {
  get: () => ipcRenderer.invoke('settings:get'),
  set: (patch) => ipcRenderer.invoke('settings:set', patch),
  chooseDownloadsFolder: () => ipcRenderer.invoke('settings:choose-downloads-folder'),
  makeDefaultBrowser: () => ipcRenderer.invoke('settings:make-default-browser'),
  newAgentToken: () => ipcRenderer.invoke('settings:new-agent-token'),
  copy: (text) => ipcRenderer.invoke('settings:copy', text),
  openBoosts: () => ipcRenderer.invoke('settings:open-boosts'),
  showTour: () => ipcRenderer.invoke('settings:show-tour'),
  onPane: (cb) => void ipcRenderer.on('settings:pane', (_e, pane) => cb(pane)),
  recordShortcut: (recording) => ipcRenderer.invoke('settings:record-shortcut', recording),
  onKey: (cb) => void ipcRenderer.on('settings:key', (_e, key) => cb(key)),
  tabs: () => ipcRenderer.invoke('settings:tabs'),
  editTabs: (edit) => ipcRenderer.invoke('settings:edit-tabs', edit),
  tabsMenu: (request) => ipcRenderer.invoke('settings:tabs-menu', request),
  onTabs: (cb) => void ipcRenderer.on('settings:tabs', (_e, view) => cb(view)),
  showTab: (tabId) => ipcRenderer.invoke('settings:show-tab', tabId),
  showEmojiPanel: () => ipcRenderer.invoke('settings:emoji-panel'),
};

contextBridge.exposeInMainWorld('settings', api);
