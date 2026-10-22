import type { SettingsApi, SettingsView } from '../shared/types';

declare global {
  interface Window {
    settings: SettingsApi;
  }
}

const api = window.settings;
const adBlock = document.getElementById('adBlock') as HTMLInputElement;

function render(view: SettingsView) {
  adBlock.checked = view.adBlock;
}

adBlock.addEventListener('change', async () => render(await api.set({ adBlock: adBlock.checked })));

api.get().then(render);
