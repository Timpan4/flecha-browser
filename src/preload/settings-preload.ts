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
  onPane: (cb) => void ipcRenderer.on('settings:pane', (_e, pane) => cb(pane)),
};

contextBridge.exposeInMainWorld('settings', api);
