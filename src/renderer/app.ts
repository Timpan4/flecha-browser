import { devSuggestions } from './devCommands.js';
import { ICONS, type IconName } from './icons.js';
import type { DownloadInfo, DropTarget, FlechaApi, HistoryEntry, PaletteMode, PeekState, PinnedNode, Space, TabInfo, UiState } from '../shared/types';

declare global {
  interface Window {
    flecha: FlechaApi;
  }
}

const api = window.flecha;
const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const DRAG_TYPE = 'application/x-flecha-tab';

let state: UiState | undefined;

// ---- tiny DOM helper --------------------------------------------------------

type Props = Record<string, unknown> & { class?: string };
function h(tag: string, props: Props = {}, ...children: Array<Node | string | null | undefined>) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value === undefined || value === false) continue;
    if (key === 'class') el.className = String(value);
    else if (key.startsWith('on')) el.addEventListener(key.slice(2).toLowerCase(), value as EventListener);
    else if (key === 'dataset') Object.assign(el.dataset, value);
    else el.setAttribute(key, value === true ? '' : String(value));
  }
  for (const child of children) if (child != null) el.append(child);
  return el;
}

function svg(path: string) {
  const el = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  el.setAttribute('viewBox', '0 0 16 16');
  el.innerHTML = `<path d="${path}"/>`;
  return el;
}

function icon(name: IconName) {
  return svg(ICONS[name]);
}

// Static markup marks icons as <i data-icon="name">; swap them for the shared SVGs.
for (const el of document.querySelectorAll<HTMLElement>('i[data-icon]')) el.replaceWith(icon(el.dataset.icon as IconName));

function host(url: string) {
  try {
    return new URL(url).host.replace(/^www\./, '');
  } catch {
    return url;
  }
}

/** A stable colour per site, so placeholder icons are told apart at a glance. */
function hueFor(text: string) {
  let hash = 0;
  for (const char of text) hash = (hash * 31 + char.charCodeAt(0)) >>> 0;
  return hash % 360;
}

function favicon(tab: TabInfo) {
  const site = host(tab.url);
  const letter = () =>
    h('span', { class: 'favicon letter', style: `--hue: ${hueFor(site)}` }, (site.replace(/^[^a-z0-9]+/i, '')[0] ?? '•').toUpperCase());
  let hostname = '';
  try {
    hostname = new URL(tab.url).hostname;
  } catch {
    // Not a web page.
  }
  // The page's own icon once it has loaded, else the icon Flecha fetched for the site.
  const src = tab.favicon || (state?.faviconHosts.includes(hostname) ? `favicon://${hostname}` : undefined);
  if (!src) return letter();
  const img = h('img', { class: 'favicon', src, draggable: 'false' }) as HTMLImageElement;
  img.onerror = () => img.replaceWith(letter());
  return img;
}

// ---- rendering ----------------------------------------------------------------

function render(s: UiState) {
  state = s;
  const space = s.spaces.find((sp) => sp.id === s.activeSpaceId) ?? s.spaces[0];
  const active = space.activeTabId ? s.tabs[space.activeTabId] : undefined;
  document.documentElement.style.setProperty('--space-color', s.tint);

  ($('btn-back') as HTMLButtonElement).disabled = !s.nav.canGoBack;
  ($('btn-forward') as HTMLButtonElement).disabled = !s.nav.canGoForward;

  renderUrlbar(s, active);

  $('favorites').replaceChildren(
    ...s.favorites
      .map((id) => s.tabs[id])
      .filter(Boolean)
      .map((tab) => {
        const el = h('button', { class: `fav${tab.id === space.activeTabId ? ' active' : ''}`, title: tab.title }, favicon(tab));
        wireTab(el, tab, { zone: 'favorites', beforeTabId: tab.id });
        return el;
      }),
  );

  renderLocalServers(s);
  renderAgent(s);

  const title = $('space-title');
  if (title.contentEditable !== 'true') title.textContent = space.name;
  $('space-emoji').textContent = space.emoji ?? '';
  $('space-emoji').hidden = !space.emoji;
  title.dataset.spaceId = space.id;
  $('space-isolated').hidden = !space.isolated;

  $('pinned').replaceChildren(...space.pinned.map((node) => renderNode(node, space, undefined)));
  $('today').replaceChildren(
    ...space.today.map((id) => s.tabs[id]).filter(Boolean).map((tab) => tabRow(tab, space, { zone: 'today', beforeTabId: tab.id })),
  );

  $('spaces').replaceChildren(
    ...s.spaces.map((sp) =>
      h(
        'button',
        {
          class: `space-dot${sp.id === space.id ? ' active' : ''}`,
          title: sp.name,
          onclick: () => api.switchSpace(sp.id),
          oncontextmenu: (e: Event) => {
            e.preventDefault();
            api.spaceMenu(sp.id);
          },
        },
        sp.emoji ? h('span', { class: 'space-emoji' }, sp.emoji) : h('span', { class: 'space-initial' }, sp.name[0]?.toUpperCase() ?? '•'),
      ),
    ),
  );

  if (!palette.hidden) updatePalette();
  if (!spaceEditor.hidden) renderSpaceEditor();
}

/** Host, a padlock or code icon for the kind of connection, and the environment badge. */
function renderUrlbar(s: UiState, active: TabInfo | undefined) {
  const url = active && active.url !== 'about:blank' ? active.url : undefined;
  let kind: IconName = 'search';
  if (url) kind = isDevUrl(url) ? 'code' : url.startsWith('https:') ? 'lock' : url.startsWith('http:') ? 'unlocked' : 'globe';
  $('urlbar-icon').replaceChildren(icon(kind));
  $('urlbar-icon').className = url?.startsWith('http:') && !isDevUrl(url) ? 'insecure' : '';
  $('urlbar-text').textContent = url ? host(url) : 'Search or enter URL';
  $('urlbar').classList.toggle('empty', !url);
  $('urlbar-env').replaceChildren(
    ...(s.environment ? [h('span', { class: `env-badge ${s.environment.tone}`, title: `${s.environment.name} · ⌥⌘E to switch` }, s.environment.label)] : []),
  );
}

/** Shows when a coding agent is connected and what it is doing right now. */
function renderAgent(s: UiState) {
  const status = $('agent-status');
  status.hidden = !s.agent?.connected;
  status.classList.toggle('busy', !!s.agent?.busy);
  $('agent-status-text').textContent = s.agent?.busy ? `Agent is ${s.agent.action}` : 'Agent connected';
}

/** Dev servers on this machine; clicking one goes to its tab, or opens one. */
function renderLocalServers(s: UiState) {
  $('local-servers').hidden = !s.localServers.length;
  $('local-server-list').replaceChildren(
    ...s.localServers.map((server) => {
      const open = Object.values(s.tabs).find((t) => originOf(t.url) === originOf(server.url));
      return h(
        'button',
        {
          class: `row local-server${open ? ' open' : ''}`,
          title: server.url,
          onclick: () => (open ? api.activate(open.id) : api.newTab(server.url)),
        },
        h('span', { class: 'live-dot' }),
        h('span', { class: 'title' }, server.name),
        h('span', { class: 'port' }, `:${server.port}`),
      );
    }),
  );
}

function originOf(url: string) {
  try {
    // localhost and 127.0.0.1 are the same server.
    return new URL(url).origin.replace('127.0.0.1', 'localhost');
  } catch {
    return url;
  }
}

function renderNode(node: PinnedNode, space: Space, folderId: string | undefined): HTMLElement {
  if (node.kind === 'tab') {
    const tab = state!.tabs[node.tabId];
    return tab ? tabRow(tab, space, { zone: 'pinned', folderId, beforeTabId: tab.id }) : h('span');
  }
  const name = h('span', { class: 'title folder-name' }, node.name);
  const header = h(
    'button',
    {
      class: 'row',
      onclick: () => api.toggleFolder(node.id),
      ondblclick: (e: Event) => {
        e.stopPropagation();
        editInline(name, node.name, (value) => api.renameFolder(node.id, value));
      },
    },
    h('span', { class: 'row-icon folder-icon' }, icon(node.open ? 'folderOpen' : 'folder')),
    name,
    h('span', { class: 'chevron' }, icon('chevron')),
  );
  acceptDrops(header, { zone: 'pinned', folderId: node.id });
  return h(
    'div',
    { class: `folder${node.open ? ' open' : ''}` },
    header,
    h('div', { class: 'folder-children' }, ...node.children.map((child) => renderNode(child, space, node.id))),
  );
}

function tabRow(tab: TabInfo, space: Space, target: DropTarget) {
  const close = h(
    'span',
    {
      class: 'close',
      title: target.zone === 'today' ? 'Close tab' : 'Unload tab',
      onclick: (e: Event) => {
        e.stopPropagation();
        api.close(tab.id);
      },
    },
    icon('close'),
  );
  const inSplit = !!space.split?.includes(tab.id) && !!space.activeTabId && space.split.includes(space.activeTabId);
  const row = h(
    'button',
    {
      class: `row${tab.id === space.activeTabId ? ' active' : ''}${inSplit ? ' in-split' : ''}${tab.loading ? ' loading' : ''}${
        state?.agent?.busy && state.agent.tabId === tab.id ? ' agent-driving' : ''
      }`,
      title: tab.url,
    },
    favicon(tab),
    h('span', { class: 'title' }, tab.title || host(tab.url)),
    errorBadge(tab),
    isBoosted(tab.url) ? h('span', { class: 'boost-mark', title: 'Boosted: a script or styles from ~/.flecha/boosts run here' }, icon('zap')) : null,
    tab.device === 'mobile'
      ? h('span', { class: 'device-badge', title: 'Phone-sized mirror' }, icon('phone'))
      : h('span', { class: 'split-badge', title: 'In split view' }, icon('columns')),
    close,
  );
  wireTab(row, tab, target);
  return row;
}

/** Red count of console errors, only on pages you're developing (local URLs). */
function errorBadge(tab: TabInfo) {
  const count = state?.consoleErrors[tab.id];
  if (!count || !isDevUrl(tab.url)) return null;
  return h(
    'span',
    {
      class: 'error-badge',
      title: `${count} console error${count === 1 ? '' : 's'} · click to open the Console`,
      onclick: (e: Event) => {
        e.stopPropagation();
        api.showConsole(tab.id);
      },
    },
    count > 99 ? '99+' : String(count),
  );
}

function isBoosted(url: string) {
  try {
    const { hostname } = new URL(url);
    return !!state?.boostedHosts.some((h) => h !== 'all' && (hostname === h || hostname.endsWith(`.${h}`)));
  } catch {
    return false;
  }
}

function isDevUrl(url: string) {
  try {
    const { hostname } = new URL(url);
    return /^(localhost|127\.0\.0\.1|\[::1\])$|\.(localhost|local|test)$/.test(hostname);
  } catch {
    return false;
  }
}

function wireTab(el: HTMLElement, tab: TabInfo, target: DropTarget) {
  el.draggable = true;
  el.addEventListener('click', () => api.activate(tab.id));
  el.addEventListener('auxclick', (e) => e.button === 1 && api.close(tab.id));
  el.addEventListener('contextmenu', (e) => {
    e.preventDefault();
    api.tabMenu(tab.id);
  });
  el.addEventListener('dragstart', (e) => {
    e.dataTransfer!.setData(DRAG_TYPE, tab.id);
    e.dataTransfer!.effectAllowed = 'move';
    $('favorites').classList.add('show-empty');
  });
  el.addEventListener('dragend', () => $('favorites').classList.remove('show-empty'));
  acceptDrops(el, target, 'drop-before');
}

function acceptDrops(el: HTMLElement, target: DropTarget, hoverClass = 'drag-over') {
  el.addEventListener('dragover', (e) => {
    if (!e.dataTransfer?.types.includes(DRAG_TYPE)) return;
    e.preventDefault();
    e.stopPropagation();
    el.classList.add(hoverClass);
  });
  el.addEventListener('dragleave', () => el.classList.remove(hoverClass));
  el.addEventListener('drop', (e) => {
    el.classList.remove(hoverClass);
    const id = e.dataTransfer?.getData(DRAG_TYPE);
    if (!id) return;
    e.preventDefault();
    e.stopPropagation();
    api.moveTab(id, target);
  });
}

function editInline(el: HTMLElement, current: string, commit: (value: string) => void) {
  el.contentEditable = 'true';
  el.textContent = current;
  el.focus();
  document.getSelection()?.selectAllChildren(el);
  const finish = (save: boolean) => {
    el.removeEventListener('keydown', onKey);
    el.removeEventListener('blur', onBlur);
    el.contentEditable = 'false';
    const value = el.textContent?.trim() ?? '';
    if (save && value && value !== current) commit(value);
    else if (state) render(state);
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === 'Enter') e.preventDefault(), finish(true);
    if (e.key === 'Escape') finish(false);
  };
  const onBlur = () => finish(true);
  el.addEventListener('keydown', onKey);
  el.addEventListener('blur', onBlur);
}

// ---- Space editor -------------------------------------------------------------------

const SPACE_EMOJIS = ['🏠', '💼', '🚀', '🧪', '🛠️', '🎨', '📚', '🧠', '💡', '🎮', '🎧', '🎬', '✈️', '🌱', '☕️', '🔥', '⭐️', '🌙', '🌊', '🐙', '🦊', '🐳', '⚡️', '🎯', '📈', '💬', '📝', '🗂️', '🛒', '❤️'];
const SPACE_COLORS = ['#cdd9f7', '#c9e3f5', '#cfe8ec', '#d3ecd5', '#d9e8c6', '#f5e7c4', '#f7dcc0', '#f6d6cc', '#f4cfdc', '#e8d6f3', '#d6d3f5', '#dfe3e8'];

const spaceEditor = $('space-editor');
const spaceName = $<HTMLInputElement>('space-name');
let editingSpace: string | undefined;

function openSpaceEditor(spaceId: string) {
  editingSpace = spaceId;
  downloadsPanel.hidden = true;
  spaceEditor.hidden = false;
  renderSpaceEditor();
  spaceName.focus();
  spaceName.select();
}

function closeSpaceEditor() {
  editingSpace = undefined;
  spaceEditor.hidden = true;
}

function renderSpaceEditor() {
  const space = state?.spaces.find((s) => s.id === editingSpace);
  if (!space) return closeSpaceEditor();
  if (document.activeElement !== spaceName) spaceName.value = space.name;
  $('space-emojis').replaceChildren(
    h('button', { class: `emoji${space.emoji ? '' : ' selected'}`, title: 'No icon', onclick: () => api.updateSpace(space.id, { emoji: '' }) }, icon('close')),
    ...SPACE_EMOJIS.map((emoji) =>
      h('button', { class: `emoji${space.emoji === emoji ? ' selected' : ''}`, onclick: () => api.updateSpace(space.id, { emoji }) }, emoji),
    ),
  );
  const custom = h('input', { type: 'color', value: space.color, title: 'Custom colour' }) as HTMLInputElement;
  custom.addEventListener('input', () => api.updateSpace(space.id, { color: custom.value }));
  $('space-colors').replaceChildren(
    ...SPACE_COLORS.map((color) =>
      h('button', {
        class: `swatch${space.color === color ? ' selected' : ''}`,
        style: `--swatch: ${color}`,
        title: color,
        onclick: () => api.updateSpace(space.id, { color }),
      }),
    ),
    h('label', { class: `swatch custom${SPACE_COLORS.includes(space.color) ? '' : ' selected'}`, style: `--swatch: ${space.color}`, title: 'Custom colour' }, custom),
  );
  $<HTMLInputElement>('space-isolated-toggle').checked = !!space.isolated;
  $<HTMLButtonElement>('space-delete').disabled = state!.spaces.length < 2;
}

spaceName.addEventListener('input', () => editingSpace && spaceName.value.trim() && api.updateSpace(editingSpace, { name: spaceName.value }));
spaceName.addEventListener('keydown', (e) => (e.key === 'Enter' || e.key === 'Escape') && closeSpaceEditor());
$('space-isolated-toggle').addEventListener('change', (e) => {
  if (editingSpace) api.setSpaceIsolated(editingSpace, (e.target as HTMLInputElement).checked);
});
$('space-delete').onclick = () => {
  if (!editingSpace) return;
  api.deleteSpace(editingSpace);
  closeSpaceEditor();
};
$('space-done').onclick = closeSpaceEditor;
document.addEventListener('mousedown', (e) => {
  const target = e.target as Node;
  if (!spaceEditor.hidden && !spaceEditor.contains(target) && !$('space-header').contains(target)) closeSpaceEditor();
});
document.addEventListener('keydown', (e) => e.key === 'Escape' && !spaceEditor.hidden && closeSpaceEditor());

// ---- command bar ----------------------------------------------------------------

interface PaletteItem {
  label: string;
  sub: string;
  run: () => void;
  /** Shown under the results while this item is selected, e.g. a decoded JWT. */
  detail?: string;
}

const palette = $('palette');
const paletteInput = $<HTMLInputElement>('palette-input');
const paletteList = $('palette-results');
let paletteMode: PaletteMode = 'new-tab';
let paletteItems: PaletteItem[] = [];
let selected = 0;

function looksLikeUrl(q: string) {
  return /^[a-z][a-z0-9+.-]*:\/\//i.test(q) || (!/\s/.test(q) && /\.[a-z]{2,}(:\d+)?(\/|$)/i.test(q)) || /^localhost(:\d+)?/.test(q);
}

function openPalette(mode: PaletteMode) {
  paletteMode = mode;
  palette.hidden = false;
  const space = state?.spaces.find((s) => s.id === state!.activeSpaceId);
  const active = space?.activeTabId ? state?.tabs[space.activeTabId] : undefined;
  paletteInput.value = mode === 'navigate' && active ? active.url : '';
  paletteInput.focus();
  paletteInput.select();
  selected = 0;
  historyResults = [];
  updatePalette();
  refreshHistory();
}

function closePalette() {
  if (palette.hidden) return;
  palette.hidden = true;
  api.closePalette();
}

function updatePalette() {
  if (!state) return;
  const q = paletteInput.value.trim();
  const lower = q.toLowerCase();
  const matches = (title: string, url: string) => !lower || title.toLowerCase().includes(lower) || url.toLowerCase().includes(lower);
  const items: PaletteItem[] = [];

  const activeId = state.spaces.find((s) => s.id === state!.activeSpaceId)?.activeTabId;
  if (q) {
    const open = { navigate: api.navigate, 'new-tab': api.newTab, split: api.splitNew }[paletteMode];
    // `env` lists the current page on the project's other environments.
    if (/^env\b/i.test(q) && state.environment) {
      for (const target of state.environment.switches) {
        items.push({ label: `Switch to ${target.label}`, sub: `${state.environment.name} · ${host(target.url)}`, run: () => api.navigate(target.url) });
      }
    }
    // `boost` edits this site's boost files.
    if (/^boost/i.test(q)) {
      const site = activeId ? host(state.tabs[activeId]?.url ?? '') : '';
      if (site) {
        items.push({ label: `Edit boost script for ${site}`, sub: 'TypeScript · runs on every page of the site', run: () => api.editBoost('script') });
        items.push({ label: `Edit boost styles for ${site}`, sub: 'CSS · applies live as you save', run: () => api.editBoost('styles') });
      }
      items.push({ label: 'Open boosts folder', sub: '~/.flecha/boosts · keep it in git', run: () => api.openBoostsFolder() });
    }
    for (const dev of devSuggestions(q, activeId ? state.tabs[activeId]?.url : undefined)) {
      items.push({
        label: dev.label,
        sub: dev.sub,
        detail: dev.copy,
        run: () => (dev.url ? open(dev.url) : api.copyText(dev.copy ?? '')),
      });
    }
    const verb = looksLikeUrl(q) ? 'Open' : `Search ${state.searchEngineName}`;
    items.push({ label: q, sub: paletteMode === 'split' ? `${verb} in split view` : verb, run: () => open(q) });
  }

  const openTabs = Object.values(state.tabs)
    .filter((t) => matches(t.title, t.url) && !(paletteMode === 'split' && t.id === activeId))
    .sort((a, b) => b.lastActive - a.lastActive)
    .slice(0, q ? 6 : 8);
  for (const tab of openTabs) {
    items.push(
      paletteMode === 'split'
        ? { label: tab.title || tab.url, sub: `Split with · ${host(tab.url)}`, run: () => api.splitWith(tab.id) }
        : { label: tab.title || tab.url, sub: `Switch to tab · ${host(tab.url)}`, run: () => api.activate(tab.id) },
    );
  }

  if (q) {
    const seen = new Set(openTabs.map((t) => t.url));
    const open = paletteMode === 'split' ? api.splitNew : paletteMode === 'navigate' ? api.navigate : api.newTab;
    for (const entry of historyResults.filter((e) => !seen.has(e.url)).slice(0, 6)) {
      seen.add(entry.url);
      items.push({ label: entry.title || entry.url, sub: `History · ${host(entry.url)}`, run: () => open(entry.url) });
    }
    for (const item of state.archive.filter((a) => !seen.has(a.url) && matches(a.title, a.url)).slice(0, 5)) {
      seen.add(item.url);
      const reopen = paletteMode === 'split' ? api.splitNew : api.newTab;
      items.push({ label: item.title || item.url, sub: `Archive · ${host(item.url)}`, run: () => reopen(item.url) });
    }
  }

  paletteItems = items;
  selected = Math.min(selected, Math.max(items.length - 1, 0));
  paletteList.replaceChildren(
    ...items.map((item, i) =>
      h(
        'li',
        {
          class: i === selected ? 'selected' : undefined,
          onmousemove: () => {
            if (selected !== i) (selected = i), updatePalette();
          },
          onmousedown: (e: Event) => {
            e.preventDefault();
            runItem(i);
          },
        },
        h('span', { class: 'label' }, item.label),
        h('span', { class: 'sub' }, item.sub),
      ),
    ),
  );
  paletteList.children[selected]?.scrollIntoView({ block: 'nearest' });
  const detail = $('palette-detail');
  detail.hidden = !items[selected]?.detail;
  detail.textContent = items[selected]?.detail ?? '';
}

function runItem(i: number) {
  const item = paletteItems[i];
  if (!item) return;
  item.run();
  closePalette();
}

// History lives in the main process; results arrive asynchronously and are dropped if stale.
let historyResults: HistoryEntry[] = [];
function refreshHistory() {
  const q = paletteInput.value.trim();
  if (!q) {
    historyResults = [];
    return;
  }
  api.searchHistory(q).then((results) => {
    if (paletteInput.value.trim() !== q || palette.hidden) return;
    historyResults = results;
    updatePalette();
  });
}

paletteInput.addEventListener('input', () => {
  selected = 0;
  updatePalette();
  refreshHistory();
});
paletteInput.addEventListener('keydown', (e) => {
  if (e.key === 'Escape') closePalette();
  else if (e.key === 'Enter') runItem(selected);
  else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    const n = paletteItems.length;
    if (n) selected = (selected + (e.key === 'ArrowDown' ? 1 : -1) + n) % n;
    updatePalette();
  } else return;
});
palette.addEventListener('mousedown', (e) => {
  if (e.target === palette) closePalette();
});
paletteInput.addEventListener('blur', () => setTimeout(() => document.activeElement !== paletteInput && closePalette(), 0));

// ---- peek -------------------------------------------------------------------------

const peek = $('peek');
const peekBar = peek.querySelector<HTMLElement>('.peek-bar')!;

function renderPeek(p: PeekState | null) {
  peek.hidden = !p;
  if (!p) return;
  const barHeight = 36;
  Object.assign(peekBar.style, {
    left: `${p.bounds.x}px`,
    top: `${p.bounds.y - barHeight - 6}px`,
    width: `${p.bounds.width}px`,
  });
  peekBar.querySelector('.peek-title')!.textContent = p.title || host(p.url);
}

peek.addEventListener('mousedown', (e) => {
  if (e.target === peek) api.closePeek();
});
$('peek-open').onclick = () => api.promotePeek();
$('peek-close').onclick = () => api.closePeek();

// ---- downloads --------------------------------------------------------------------

const downloadsPanel = $('downloads-panel');

function formatBytes(n: number) {
  if (n < 1024) return `${n} B`;
  const units = ['KB', 'MB', 'GB'];
  let value = n / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) (value /= 1024), unit++;
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

function downloadStatus(d: DownloadInfo) {
  if (d.state === 'completed') return formatBytes(d.received);
  if (d.state === 'cancelled') return 'Cancelled';
  if (d.state === 'interrupted') return 'Failed';
  return d.total ? `${formatBytes(d.received)} of ${formatBytes(d.total)}` : formatBytes(d.received);
}

function renderDownloads(items: DownloadInfo[], started?: DownloadInfo) {
  const active = items.filter((d) => d.state === 'progressing').length;
  const badge = $('downloads-badge');
  badge.hidden = !active;
  badge.textContent = String(active);
  if (started) downloadsPanel.hidden = false;

  $('downloads-list').replaceChildren(
    ...items.map((d) => {
      const progress = d.state === 'progressing' && d.total ? d.received / d.total : d.state === 'completed' ? 1 : 0;
      return h(
        'li',
        {
          class: `download ${d.state}`,
          title: d.state === 'completed' ? 'Open' : d.url,
          onclick: () => api.openDownload(d.id),
        },
        h('span', { class: 'name' }, d.filename),
        h('span', { class: 'status' }, downloadStatus(d)),
        d.state === 'progressing'
          ? h('span', { class: 'bar' }, h('span', { class: 'fill', style: `width: ${Math.round(progress * 100)}%` }))
          : null,
        h(
          'button',
          {
            class: 'icon-button',
            title: d.state === 'progressing' ? 'Cancel' : 'Show in Finder',
            onclick: (e: Event) => {
              e.stopPropagation();
              if (d.state === 'progressing') api.cancelDownload(d.id);
              else api.revealDownload(d.id);
            },
          },
          d.state === 'progressing' ? icon('close') : icon('folder'),
        ),
      );
    }),
  );
  if (!items.length) $('downloads-list').append(h('li', { class: 'empty' }, 'No downloads yet'));
}

$('btn-downloads').onclick = () => (downloadsPanel.hidden = !downloadsPanel.hidden);
$('btn-clear-downloads').onclick = () => api.clearDownloads();
document.addEventListener('mousedown', (e) => {
  const target = e.target as Node;
  if (!downloadsPanel.hidden && !downloadsPanel.contains(target) && !$('btn-downloads').contains(target)) {
    downloadsPanel.hidden = true;
  }
});

// ---- wiring ---------------------------------------------------------------------

$('btn-sidebar').onclick = () => api.toggleSidebar();
$('btn-back').onclick = () => api.back();
$('btn-forward').onclick = () => api.forward();
$('btn-reload').onclick = () => api.reload();
$('urlbar').onclick = () => api.openPalette('navigate');
$('btn-newtab').onclick = () => api.openPalette('new-tab');
$('btn-clear').onclick = () => api.clearToday();
$('btn-newspace').onclick = () => api.newSpace();
$('space-header').ondblclick = () => state && openSpaceEditor(state.activeSpaceId);
$('space-header').oncontextmenu = (e) => {
  e.preventDefault();
  if (state) api.spaceMenu(state.activeSpaceId);
};

acceptDrops($('favorites'), { zone: 'favorites' });
acceptDrops($('pinned'), { zone: 'pinned' });
acceptDrops($('today'), { zone: 'today' });
acceptDrops($('btn-newtab'), { zone: 'today' });

api.onState(render);
api.onPalette(openPalette);
api.onEditSpace(openSpaceEditor);
api.onPeek(renderPeek);
api.onDownloads(renderDownloads);
api.ready();
