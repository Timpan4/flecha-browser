import type { SettingsApi, SettingsView } from '../shared/types';

declare global {
  interface Window {
    settings: SettingsApi;
  }
}

const api = window.settings;
const adBlock = document.getElementById('adBlock') as HTMLInputElement;
const searchEngine = document.getElementById('searchEngine') as HTMLSelectElement;

function render(view: SettingsView) {
  adBlock.checked = view.adBlock;
  if (searchEngine.options.length !== view.searchEngines.length) {
    searchEngine.replaceChildren(...view.searchEngines.map((e) => new Option(e.name, e.id)));
  }
  searchEngine.value = view.searchEngine;
}

searchEngine.addEventListener('change', async () =>
  render(await api.set({ searchEngine: searchEngine.value as SettingsView['searchEngine'] })),
);
adBlock.addEventListener('change', async () => render(await api.set({ adBlock: adBlock.checked })));

api.get().then(render);
