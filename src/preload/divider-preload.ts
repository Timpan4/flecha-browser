import { contextBridge, ipcRenderer } from 'electron';
import type { DividerApi } from '../shared/types';

const api: DividerApi = {
  start: () => ipcRenderer.send('divider:start'),
  drag: (clientX) => ipcRenderer.send('divider:drag', clientX),
  end: () => ipcRenderer.send('divider:end'),
  reset: () => ipcRenderer.send('divider:reset'),
};

contextBridge.exposeInMainWorld('divider', api);
