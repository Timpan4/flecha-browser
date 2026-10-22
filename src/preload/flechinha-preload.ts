import { contextBridge, ipcRenderer } from 'electron';
import type { FlechinhaApi } from '../shared/types';

const cmd =
  (name: string) =>
  (...args: unknown[]) =>
    ipcRenderer.send('flechinha:cmd', name, ...args);

const api: FlechinhaApi = {
  onState: (cb) => void ipcRenderer.on('flechinha:state', (_e, state) => cb(state)),
  onEditAddress: (cb) => void ipcRenderer.on('flechinha:edit-address', () => cb()),
  onFlash: (cb) => void ipcRenderer.on('flechinha:flash', (_e, message) => cb(message)),
  ready: cmd('ready'),
  back: cmd('back'),
  forward: cmd('forward'),
  reload: cmd('reload'),
  stop: cmd('stop'),
  navigate: cmd('navigate'),
  copyLink: cmd('copyLink'),
  openInFlecha: cmd('openInFlecha'),
  close: cmd('close'),
};

contextBridge.exposeInMainWorld('flechinha', api);
