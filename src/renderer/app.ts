import { devSuggestions } from './devCommands.js';
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

function host(url: string) {
  try {
    return new URL(url).host.replace(/^www\./, '');
  } catch {
    return url;
  }
}

function favicon(tab: TabInfo) {
  const letter = () => h('span', { class: 'favicon letter' }, (host(tab.url)[0] ?? '?').toUpperCase());
  if (!tab.favicon) return letter();
  const img = h('img', { class: 'favicon', src: tab.favicon, draggable: 'false' }) as HTMLImageElement;
  img.onerror = () => img.replaceWith(letter());
  return img;
}

// ---- rendering ----------------------------------------------------------------

function render(s: UiState) {
  state = s;
  const space = s.spaces.find((sp) => sp.id === s.activeSpaceId) ?? s.spaces[0];
  const active = space.activeTabId ? s.tabs[space.activeTabId] : undefined;
  document.documentElement.style.setProperty('--space-color', space.color);

  ($('btn-back') as HTMLButtonElement).disabled = !s.nav.canGoBack;
  ($('btn-forward') as HTMLButtonElement).disabled = !s.nav.canGoForward;

  const urlbar = $('urlbar');
  urlbar.replaceChildren(active && active.url !== 'about:blank' ? host(active.url) : 'Search or enter URL…');
  if (s.environment) {
    urlbar.prepend(h('span', { class: `env-badge ${s.environment.tone}`, title: `${s.environment.name} · ⌥⌘E to switch` }, s.environment.label));
  }
  urlbar.classList.toggle('has-url', !!active);

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

  const title = $('space-title');
  if (title.contentEditable !== 'true') title.textContent = `${space.emoji ? space.emoji + '  ' : ''}${space.name}`;
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
        sp.emoji || sp.name[0]?.toUpperCase() || '•',
      ),
    ),
  );

  if (!palette.hidden) updatePalette();
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
    h('span', { class: 'chevron' }, svg('m6 4 4 4-4 4')),
    name,
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
    svg('M4 4l8 8M12 4l-8 8'),
  );
  const inSplit = !!space.split?.includes(tab.id) && !!space.activeTabId && space.split.includes(space.activeTabId);
  const row = h(
    'button',
    {
      class: `row${tab.id === space.activeTabId ? ' active' : ''}${inSplit ? ' in-split' : ''}${tab.loading ? ' loading' : ''}`,
      title: tab.url,
    },
    favicon(tab),
    h('span', { class: 'title' }, tab.title || host(tab.url)),
    errorBadge(tab),
    tab.device === 'mobile'
      ? h('span', { class: 'device-badge', title: 'Phone-sized mirror' }, svg('M5 1.5h6a1 1 0 0 1 1 1v11a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1v-11a1 1 0 0 1 1-1zM7 12.5h2'))
      : h('span', { class: 'split-badge', title: 'In split view' }, svg('M2.5 3.5h11v9h-11zM8 3.5v9')),
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

function renameSpace(spaceId: string) {
  const space = state?.spaces.find((s) => s.id === spaceId);
  if (!space) return;
  if (state!.activeSpaceId !== spaceId) api.switchSpace(spaceId);
  editInline($('space-title'), space.name, (name) => api.renameSpace(spaceId, name));
}

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
            class: 'icon',
            title: d.state === 'progressing' ? 'Cancel' : 'Show in Finder',
            onclick: (e: Event) => {
              e.stopPropagation();
              if (d.state === 'progressing') api.cancelDownload(d.id);
              else api.revealDownload(d.id);
            },
          },
          d.state === 'progressing' ? svg('M4 4l8 8M12 4l-8 8') : svg('M2 4h4l1.5 2H14v6.5H2z'),
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
$('btn-import').onclick = () => api.importArc();
$('space-title').ondblclick = () => state && renameSpace(state.activeSpaceId);
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
api.onRenameSpace(renameSpace);
api.onPeek(renderPeek);
api.onDownloads(renderDownloads);
api.ready();
