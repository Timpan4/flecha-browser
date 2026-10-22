import type { DropTarget, FlechaApi, PaletteMode, PeekState, PinnedNode, Space, TabInfo, UiState } from '../shared/types';

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
  urlbar.textContent = active && active.url !== 'about:blank' ? host(active.url) : 'Search or enter URL…';
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

  const title = $('space-title');
  if (title.contentEditable !== 'true') title.textContent = `${space.emoji ? space.emoji + '  ' : ''}${space.name}`;
  title.dataset.spaceId = space.id;

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
    h('span', { class: 'split-badge', title: 'In split view' }, svg('M2.5 3.5h11v9h-11zM8 3.5v9')),
    close,
  );
  wireTab(row, tab, target);
  return row;
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
  updatePalette();
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

  if (q) {
    const open = { navigate: api.navigate, 'new-tab': api.newTab, split: api.splitNew }[paletteMode];
    const verb = looksLikeUrl(q) ? 'Open' : 'Search Google';
    items.push({ label: q, sub: paletteMode === 'split' ? `${verb} in split view` : verb, run: () => open(q) });
  }

  const activeId = state.spaces.find((s) => s.id === state!.activeSpaceId)?.activeTabId;
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
}

function runItem(i: number) {
  const item = paletteItems[i];
  if (!item) return;
  item.run();
  closePalette();
}

paletteInput.addEventListener('input', () => {
  selected = 0;
  updatePalette();
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
api.ready();
