import { contextBridge, ipcRenderer } from 'electron';
import type { FindBarApi } from '../shared/types';

const api: FindBarApi = {
  onOpen: (cb) => void ipcRenderer.on('find:open', () => cb()),
  onResult: (cb) => void ipcRenderer.on('find:result', (_e, result) => cb(result)),
  find: (text, forward, newSearch) => ipcRenderer.send('find:query', text, forward, newSearch),
  close: () => ipcRenderer.send('find:close'),
};

contextBridge.exposeInMainWorld('findBar', api);
