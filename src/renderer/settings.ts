import type { SettingsApi, SettingsView } from '../shared/types';
import { ICONS, type IconName } from './icons.js';

declare global {
  interface Window {
    settings: SettingsApi;
  }
}

const api = window.settings;

function icon(name: IconName) {
  const el = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  el.setAttribute('viewBox', '0 0 16 16');
  el.innerHTML = `<path d="${ICONS[name]}"/>`;
  return el;
}

for (const el of document.querySelectorAll<HTMLElement>('i[data-icon]')) el.replaceWith(icon(el.dataset.icon as IconName));

// ---- navigation: one pane at a time, like System Settings ----------------------

function showPane(name: string) {
  const item = document.querySelector<HTMLElement>(`.nav-item[data-pane="${name}"]`) ?? document.querySelector<HTMLElement>('.nav-item')!;
  for (const el of document.querySelectorAll<HTMLElement>('.nav-item')) el.classList.toggle('selected', el === item);
  for (const pane of document.querySelectorAll<HTMLElement>('.pane')) pane.hidden = pane.dataset.pane !== item.dataset.pane;
  document.getElementById('pane-title')!.textContent = item.textContent!.trim();
  try {
    localStorage.setItem('pane', item.dataset.pane!);
  } catch {
    // Remembering the pane is a nicety.
  }
}

for (const item of document.querySelectorAll<HTMLElement>('.nav-item')) item.addEventListener('click', () => showPane(item.dataset.pane!));
let lastPane = 'general';
try {
  lastPane = localStorage.getItem('pane') ?? lastPane;
} catch {
  // Private storage can be unavailable.
}
// A pane asked for by the menu (Help → Keyboard Shortcuts) wins over the last one used.
showPane(location.hash.slice(1) || lastPane);
api.onPane(showPane);
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
      card.className = 'card env-card';
      const name = Object.assign(document.createElement('input'), { value: env.name, placeholder: 'Project name', className: 'env-name' });
      const targets = Object.assign(document.createElement('textarea'), {
        value: env.targetsText,
        placeholder: 'local http://localhost:3000\nstaging https://staging.example.com\nprod https://example.com',
        rows: 3,
        spellcheck: false,
        className: 'env-targets',
      });
      const remove = Object.assign(document.createElement('button'), { className: 'icon-button', title: 'Remove project' });
      remove.append(icon('close'));
      name.addEventListener('change', () => saveEnvironments({ ...env, name: name.value }, i));
      targets.addEventListener('change', () => saveEnvironments({ ...env, targetsText: targets.value }, i));
      remove.addEventListener('click', () => {
        environments.splice(i, 1);
        void persistEnvironments();
      });
      const header = document.createElement('div');
      header.className = 'env-header';
      header.append(name, remove);
      card.append(header, targets);
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
  environmentList.querySelector<HTMLInputElement>('.env-card:last-child .env-name')?.focus();
});

const agentEnabled = document.getElementById('agentEnabled') as HTMLInputElement;
let agent: SettingsView['agent'] = { enabled: false };

function claudeCommand() {
  return `claude mcp add --transport http flecha ${agent.endpoint} --header "Authorization: Bearer ${agent.token}"`;
}

function agentJson() {
  return JSON.stringify(
    { mcpServers: { flecha: { type: 'http', url: agent.endpoint, headers: { Authorization: `Bearer ${agent.token}` } } } },
    null,
    2,
  );
}

async function copied(button: HTMLButtonElement, text: string) {
  await api.copy(text);
  const label = button.textContent;
  button.textContent = 'Copied';
  setTimeout(() => (button.textContent = label), 1200);
}

agentEnabled.addEventListener('change', async () => render(await api.set({ agentEnabled: agentEnabled.checked })));
document.getElementById('copyClaudeCommand')!.addEventListener('click', (e) => copied(e.currentTarget as HTMLButtonElement, claudeCommand()));
document.getElementById('copyAgentJson')!.addEventListener('click', (e) => copied(e.currentTarget as HTMLButtonElement, agentJson()));
document.getElementById('newAgentToken')!.addEventListener('click', async () => render(await api.newAgentToken()));

function render(view: SettingsView) {
  adBlock.checked = view.adBlock;
  if (searchEngine.options.length !== view.searchEngines.length) {
    searchEngine.replaceChildren(...view.searchEngines.map((e) => new Option(e.name, e.id)));
  }
  searchEngine.value = view.searchEngine;
  downloadsFolder.textContent = view.downloadsFolder;
  downloadsFolder.title = view.downloadsFolder;
  askWhereToSave.checked = view.askWhereToSave;
  agent = view.agent;
  agentEnabled.checked = agent.enabled;
  for (const row of document.querySelectorAll<HTMLElement>('[data-agent-only]')) row.hidden = !agent.enabled;
  document.getElementById('agentEndpoint')!.textContent = agent.endpoint ?? 'Starting…';
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

document.getElementById('openBoosts')!.addEventListener('click', () => api.openBoosts());
document.getElementById('chooseFolder')!.addEventListener('click', async () => render(await api.chooseDownloadsFolder()));
askWhereToSave.addEventListener('change', async () => render(await api.set({ askWhereToSave: askWhereToSave.checked })));

searchEngine.addEventListener('change', async () =>
  render(await api.set({ searchEngine: searchEngine.value as SettingsView['searchEngine'] })),
);
adBlock.addEventListener('change', async () => render(await api.set({ adBlock: adBlock.checked })));

api.get().then(render);
