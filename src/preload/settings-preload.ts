import { contextBridge, ipcRenderer } from 'electron';
import type { SettingsApi } from '../shared/types';

const api: SettingsApi = {
  get: () => ipcRenderer.invoke('settings:get'),
  set: (patch) => ipcRenderer.invoke('settings:set', patch),
  chooseDownloadsFolder: () => ipcRenderer.invoke('settings:choose-downloads-folder'),
};

contextBridge.exposeInMainWorld('settings', api);
