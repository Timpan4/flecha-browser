import type { SettingsApi, SettingsView } from '../shared/types';

declare global {
  interface Window {
    settings: SettingsApi;
  }
}

const api = window.settings;
const adBlock = document.getElementById('adBlock') as HTMLInputElement;
const searchEngine = document.getElementById('searchEngine') as HTMLSelectElement;
const downloadsFolder = document.getElementById('downloadsFolder')!;
const askWhereToSave = document.getElementById('askWhereToSave') as HTMLInputElement;
const makeDefault = document.getElementById('makeDefault') as HTMLButtonElement;
const defaultStatus = document.getElementById('defaultBrowserStatus')!;

type EnvironmentDraft = SettingsView['environments'][number];
let environments: EnvironmentDraft[] = [];
const environmentList = document.getElementById('environmentList')!;

function renderEnvironments() {
  environmentList.replaceChildren(
    ...environments.map((env, i) => {
      const card = document.createElement('div');
      card.className = 'env-card';
      const name = Object.assign(document.createElement('input'), { value: env.name, placeholder: 'Project name' });
      const targets = Object.assign(document.createElement('textarea'), {
        value: env.targetsText,
        placeholder: 'local http://localhost:3000\nstaging https://staging.example.com\nprod https://example.com',
        rows: 3,
        spellcheck: false,
      });
      const remove = Object.assign(document.createElement('button'), { textContent: 'Remove' });
      name.addEventListener('change', () => saveEnvironments({ ...env, name: name.value }, i));
      targets.addEventListener('change', () => saveEnvironments({ ...env, targetsText: targets.value }, i));
      remove.addEventListener('click', () => {
        environments.splice(i, 1);
        void persistEnvironments();
      });
      card.append(name, targets, remove);
      return card;
    }),
  );
}

function saveEnvironments(updated: EnvironmentDraft, index: number) {
  environments[index] = updated;
  void persistEnvironments();
}

async function persistEnvironments() {
  // Drafts without a valid URL yet stay on screen; the main process only keeps complete ones.
  const drafts = environments;
  const view = await api.set({ environments: drafts });
  environments = drafts.map((d) => view.environments.find((e) => e.id === d.id) ?? d);
  renderEnvironments();
}

document.getElementById('addEnvironment')!.addEventListener('click', () => {
  environments.push({ id: crypto.randomUUID(), name: '', targetsText: '' });
  renderEnvironments();
  environmentList.querySelector<HTMLInputElement>('.env-card:last-child input')?.focus();
});

function render(view: SettingsView) {
  adBlock.checked = view.adBlock;
  if (searchEngine.options.length !== view.searchEngines.length) {
    searchEngine.replaceChildren(...view.searchEngines.map((e) => new Option(e.name, e.id)));
  }
  searchEngine.value = view.searchEngine;
  downloadsFolder.textContent = view.downloadsFolder;
  downloadsFolder.title = view.downloadsFolder;
  askWhereToSave.checked = view.askWhereToSave;
  if (!environments.length) {
    environments = view.environments;
    renderEnvironments();
  }

  const { isDefault, canSet } = view.defaultBrowser;
  makeDefault.hidden = isDefault;
  makeDefault.disabled = !canSet;
  defaultStatus.textContent = isDefault
    ? 'Flecha is your default browser.'
    : canSet
      ? 'Links from other apps open in your current default browser.'
      : 'Available in the packaged Flecha app, not when running from npm start.';
}

makeDefault.addEventListener('click', async () => render(await api.makeDefaultBrowser()));
// macOS asks for confirmation in its own dialog, so check again when the window regains focus.
window.addEventListener('focus', async () => render(await api.get()));

document.getElementById('chooseFolder')!.addEventListener('click', async () => render(await api.chooseDownloadsFolder()));
askWhereToSave.addEventListener('change', async () => render(await api.set({ askWhereToSave: askWhereToSave.checked })));

searchEngine.addEventListener('change', async () =>
  render(await api.set({ searchEngine: searchEngine.value as SettingsView['searchEngine'] })),
);
adBlock.addEventListener('change', async () => render(await api.set({ adBlock: adBlock.checked })));

api.get().then(render);
