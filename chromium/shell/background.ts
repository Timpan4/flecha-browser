/**
 * The shell's background: Flecha's Store, kept in step with real Chromium tabs.
 *
 * The Store is the source of truth, as in Electron. A Flecha tab only gets a Chromium tab
 * once it is opened (like Electron only made a view for it then), so a sidebar of a hundred
 * pinned tabs costs nothing until you use them. Chromium tabs that pages open themselves
 * (links to a new tab, window.open) become Flecha tabs.
 */
import { Store, type StatePersistence } from '../../src/main/store';
import { resolveInput, searchEngine } from '../../src/main/url';
import { shortcutHints, type ShortcutId } from '../../src/main/shortcuts';
import { describe as describeEnvironment } from '../../src/main/environments';
import type { BrowserState, DropTarget, SearchEngineId, UiState } from '../../src/shared/types';
import { broadcast, call, handle, listen, onConnect } from './hub';
import type { MenuRun } from './protocol';
import type { MenuEntry } from './settingsUi';
import { openSettings, registerSettings } from './settingsHandlers';
import { registerNativeHost } from './nativeHost';
import { SiteIcons } from './siteIcons';
// Loaded on the first turn: its modules add webNavigation and message listeners as they load.
import { registerFeatures, type Features } from './features';
// Also loaded on the first turn, for the shortcut event that may have woken the background.
import { defaultBrowserState, isFlechinha, isFlechinhaWindow, registerNative } from './native';
import { registerDownloads } from './downloads';
import { registerSiteControls } from './siteControls';
import { Favicons } from './favicons';

const STATE_KEY = 'state';
/** Flecha tab id ↔ Chromium tab id, for the tabs that are open. Kept across background restarts. */
const LIVE_KEY = 'live';
const ARCHIVE_ALARM = 'archive-stale';

const live = new Map<string, number>();
const flechaIdFor = (chromeId: number) => [...live].find(([, c]) => c === chromeId)?.[0];
/** Chromium tabs we're closing ourselves, so their events aren't taken as the user's. */
const ours = new Set<number>();
/** Tabs we're creating right now: Chromium may announce them before create() returns their id. */
let creating = 0;
/** A blank tab that keeps the window open while Flecha has no tab open in it. */
let placeholder: number | undefined;

async function start() {
  const saved = (await chrome.storage.local.get(STATE_KEY))[STATE_KEY] as string | undefined;
  const persistence: StatePersistence = {
    load: () => (saved ? (JSON.parse(saved) as BrowserState) : undefined),
    save: (json) => void chrome.storage.local.set({ [STATE_KEY]: json }),
  };
  const store = new Store(persistence);
  let nativeHost: ReturnType<typeof registerNativeHost> | undefined;
  let features: Features | undefined;
  // Asked of macOS on start and whenever a window takes focus (the user may have changed it in System Settings).
  let defaultBrowser: UiState['defaultBrowser'] = { isDefault: false, canSet: false };
  const refreshDefaultBrowser = async () => {
    const next = await defaultBrowserState().catch(() => defaultBrowser);
    if (next.isDefault === defaultBrowser.isDefault && next.canSet === defaultBrowser.canSet) return;
    defaultBrowser = next;
    sendState();
  };
  const restored = (await chrome.storage.session.get(LIVE_KEY))[LIVE_KEY] as Record<string, number> | undefined;
  const existing = new Set((await chrome.tabs.query({})).map((t) => t.id));
  for (const [id, chromeId] of Object.entries(restored ?? {})) if (existing.has(chromeId) && store.state.tabs[id]) live.set(id, chromeId);
  if (!restored) store.archiveStale();
  // Tabs Chromium opened before we were running: its start page shows the active Flecha tab
  // (closing it would close the window), anything else becomes a Flecha tab.
  for (const tab of await chrome.tabs.query({})) {
    if ([...live.values()].includes(tab.id!) || isShellPage(tab)) continue;
    const active = store.activeTab;
    if (isStartPage(tab) && active && !live.has(active.id)) {
      live.set(active.id, tab.id!);
      // A blank tab reloads the new tab page: Chromium drew its own before the shell was loaded.
      void chrome.tabs.update(tab.id!, { url: active.url === 'about:blank' ? 'chrome://newtab/' : active.url, active: true });
    } else if (isStartPage(tab) && !active) {
      placeholder = tab.id;
      void chrome.tabs.update(tab.id!, { url: 'chrome://newtab/' });
    } else if (isStartPage(tab)) {
      ours.add(tab.id!);
      void chrome.tabs.remove(tab.id!);
    } else live.set(addTab(url(tab), { activate: tab.active }).id, tab.id!);
  }

  /** A Flecha tab for an address; the new tab page and empty addresses make a blank "New Tab". */
  function addTab(address: string, opts: Parameters<Store['addTab']>[1]) {
    const blank = !address || address === 'about:blank' || isStartPage({ url: address } as chrome.tabs.Tab);
    const tab = store.addTab(blank ? 'about:blank' : address, opts);
    if (blank) store.updateTab(tab.id, { title: 'New Tab' });
    return tab;
  }

  // Icons fetched from the sites themselves, for those Chromium has none of yet.
  const siteIcons = new SiteIcons((icons) => broadcast('site-icons', icons));
  const favicons = new Favicons(
    () => sendStateSoon(),
    (hosts) => void siteIcons.want(hosts),
  );
  const rememberLive = () => void chrome.storage.session.set({ [LIVE_KEY]: Object.fromEntries(live) });

  function uiState(): UiState {
    const tabs = Object.fromEntries(Object.entries(store.state.tabs).map(([id, { history, ...tab }]) => [id, tab]));
    return {
      ...store.state,
      tabs,
      searchEngineName: searchEngine(store.state.searchEngine).name,
      // The light tint; pages darken it themselves, since only they know the system appearance.
      tint: store.activeSpace.color,
      sidebarMode: store.state.sidebarVisible ? 'docked' : frame.sidebarPeek ? 'floating' : 'hidden',
      environment: store.activeTab ? describeEnvironment(store.activeTab.url, store.state.environments) : undefined,
      localServers: features?.localServers() ?? [],
      consoleErrors: features?.consoleErrors() ?? {},
      boostedHosts: features?.boostedHosts() ?? [],
      faviconHosts: favicons.list(),
      faviconUrl: favicons.url,
      shortcuts: shortcutHints(store.state),
      defaultBrowser,
      agent: nativeHost?.agentActivity(),
      nav: navState,
    };
  }

  let navState: UiState['nav'] = { canGoBack: false, canGoForward: false, zoom: 100 };
  const sendState = () => {
    broadcast('state', uiState());
    sendFrame();
  };

  // ---- the window frame, as the Electron spike's Browser.layout() --------------
  // The frame page applies it (chrome.flechaPrivate.setFrame), since only a page knows
  // whether the system is in dark mode for the background colour.
  const frame = { paletteOpen: false, sidebarPeek: false, tabDragging: false, overlays: new Set<string>() };
  function sendFrame() {
    const sidebar: chrome.flechaPrivate.SidebarMode = store.state.sidebarVisible ? 'docked' : frame.sidebarPeek ? 'floating' : 'hidden';
    // The frame covers the whole window while it draws over the page.
    const overlay = frame.paletteOpen || frame.tabDragging || frame.overlays.size > 0 || !store.state.onboarded;
    broadcast('frame', { sidebar, overlay });
  }
  const hasFrame = () => typeof chrome.flechaPrivate?.setFrame === 'function';
  /** During the welcome tour, a shortcut is reported to it instead of acting. Returns whether it was. */
  function tourKey(command: string | undefined) {
    if (store.state.onboarded || !command) return false;
    broadcast('tour-key', command);
    return true;
  }

  function openPalette(mode: string) {
    // The tour asks you to try the real shortcuts and shows what they do.
    if (tourKey(({ 'new-tab': 'newTab', navigate: 'openLocation', split: 'splitNew' } as Record<string, string>)[mode])) return;
    frame.paletteOpen = true;
    sendFrame();
    broadcast('palette', mode);
    if (hasFrame()) void chrome.flechaPrivate.focus('frame');
  }

  /** The window Flecha's tabs live in: the last focused normal window that isn't a Flechinha. */
  async function mainWindow() {
    const last = await chrome.windows.getLastFocused({ windowTypes: ['normal'] }).catch(() => undefined);
    if (last?.id !== undefined && !isFlechinha(last.id)) return last.id;
    const other = (await chrome.windows.getAll({ windowTypes: ['normal'] })).find((w) => !isFlechinha(w.id));
    return other?.id ?? (await chrome.windows.create({ focused: true }))!.id!;
  }

  async function openChromeTab(id: string, active: boolean) {
    const tab = store.state.tabs[id];
    creating++;
    try {
      const created = await chrome.tabs.create({ windowId: await mainWindow(), url: tab.url === 'about:blank' ? undefined : tab.url, active });
      live.set(id, created.id!);
      if (tab.muted) void chrome.tabs.update(created.id!, { muted: true });
      rememberLive();
      return created.id!;
    } finally {
      creating--;
    }
  }

  /**
   * Closes the Chromium tabs of Flecha tabs that were closed or unloaded. Chromium closes a
   * window with its last tab, so when none would be left a blank placeholder takes its place
   * (Flecha shows its empty state over it), until a Flecha tab opens again.
   */
  async function unload(ids: string[]) {
    const closing = ids.map((id) => live.get(id)).filter((c): c is number => c !== undefined);
    for (const id of ids) live.delete(id);
    rememberLive();
    if (!closing.length) return;
    const left = (await chrome.tabs.query({ windowId: await mainWindow() })).filter((t) => !closing.includes(t.id!));
    if (!left.length && placeholder === undefined) {
      creating++;
      try {
        placeholder = (await chrome.tabs.create({ windowId: await mainWindow(), active: true })).id;
      } finally {
        creating--;
      }
    }
    for (const chromeId of closing) {
      ours.add(chromeId);
      void chrome.tabs.remove(chromeId).catch(() => ours.delete(chromeId));
    }
  }

  /** The placeholder goes once a real tab is showing. */
  function dropPlaceholder() {
    if (placeholder === undefined) return;
    ours.add(placeholder);
    void chrome.tabs.remove(placeholder).catch(() => undefined);
    placeholder = undefined;
  }

  /** Shows the active Flecha tab, opening its Chromium tab if it has none yet. */
  let syncing = Promise.resolve();
  function sync() {
    syncing = syncing
      .then(async () => {
        await unload([...live.keys()].filter((id) => !store.state.tabs[id]));
        const active = store.activeTab;
        if (!active) return;
        // Both halves of a split are on screen, so both need a page.
        for (const id of store.visibleTabIds()) if (id !== active.id && !live.has(id)) await openChromeTab(id, false);
        const chromeId = live.get(active.id);
        if (chromeId === undefined) await openChromeTab(active.id, true);
        await syncSplits();
        await chrome.tabs.update(live.get(active.id)!, { active: true }).catch(() => openChromeTab(active.id, true));
        dropPlaceholder();
        await refreshNav();
      })
      .catch((err) => console.error('Flecha shell: sync failed', err));
  }

  // ---- split view: the Store's splits as Chromium's own split view ---------------
  // Chromium shows both tabs of a split whenever either is active, which is how a Space's
  // split behaves in the Store too. Chromium places the halves by tab order.
  // TODO(chromium): the split ratio (store.splitRatio, the divider) has no extension API; a
  // native patch would apply it, and report drags of Chromium's own divider back.

  /** Split changes we asked Chromium for, by tab: their events are ours, not the user's. */
  const ourSplits = new Map<number, 'split' | 'unsplit'>();

  async function syncSplits() {
    const wanted = new Map<string, [number, number]>();
    for (const space of store.state.spaces) {
      const pair = space.split?.map((id) => live.get(id));
      if (pair?.length === 2 && pair[0] !== undefined && pair[1] !== undefined) wanted.set(pair.join(','), pair as [number, number]);
    }
    const tabs = await chrome.tabs.query({ windowId: await mainWindow() });
    const splits = new Map<number, chrome.tabs.Tab[]>();
    for (const tab of tabs) {
      if (tab.splitViewId === undefined || tab.splitViewId < 0) continue;
      splits.set(tab.splitViewId, [...(splits.get(tab.splitViewId) ?? []), tab]);
    }
    const have = new Set<string>();
    for (const [splitId, members] of splits) {
      const key = members
        .sort((a, b) => a.index - b.index)
        .map((t) => t.id)
        .join(',');
      if (wanted.has(key)) have.add(key);
      else {
        for (const t of members) ourSplits.set(t.id!, 'unsplit');
        await chrome.tabs.unsplit(splitId).catch(() => undefined);
      }
    }
    for (const [key, [left, right]] of wanted) {
      if (have.has(key)) continue;
      ourSplits.set(left, 'split');
      ourSplits.set(right, 'split');
      // Split tabs must be next to each other, left first.
      await chrome.tabs.move(left, { index: -1 }).catch(() => undefined);
      await chrome.tabs.move(right, { index: -1 }).catch(() => undefined);
      await chrome.tabs.createSplit([left, right]).catch((err) => {
        ourSplits.delete(left);
        ourSplits.delete(right);
        console.warn('Flecha shell: split failed', err);
      });
    }
  }

  /** A split made or ended in Chromium itself (its split menu, the close button) goes to the Store. */
  async function splitChanged(chromeId: number, splitViewId: number) {
    const ours = ourSplits.get(chromeId);
    if (ours && ours === (splitViewId < 0 ? 'unsplit' : 'split')) return void ourSplits.delete(chromeId);
    const id = flechaIdFor(chromeId);
    const space = store.activeSpace;
    if (!id) return;
    if (splitViewId < 0) {
      if (space.split?.includes(id)) exitSplit();
      return;
    }
    const members = (await chrome.tabs.query({ windowId: await mainWindow() }))
      .filter((t) => t.splitViewId === splitViewId)
      .sort((a, b) => a.index - b.index)
      .map((t) => flechaIdFor(t.id!));
    const active = space.activeTabId;
    if (members.length !== 2 || !members.every(Boolean) || !active || !members.includes(active)) return;
    if (space.split && members.every((m) => space.split!.includes(m!))) return;
    const other = members.find((m) => m !== active)!;
    store.splitWith(other, members[0] === other ? 'left' : 'right');
  }

  function exitSplit() {
    const mirror = store.unsplit();
    if (mirror) {
      store.closeTab(mirror);
      void unload([mirror]);
    }
  }

  /** A tab dropped on the page: show it beside the page, on the side it was dropped. */
  function dropToSplit(tabId: string, side: 'left' | 'right') {
    frame.tabDragging = false;
    sendFrame();
    const space = store.activeSpace;
    if (!space.activeTabId || space.activeTabId === tabId) return store.activate(tabId);
    // A responsive mirror only exists for its split; it goes away when something else takes its place.
    if (space.splitMode === 'responsive') exitSplit();
    store.splitWith(tabId, side);
  }

  async function refreshNav() {
    const chromeId = store.activeTab && live.get(store.activeTab.id);
    const zoom = chromeId === undefined ? 1 : await chrome.tabs.getZoom(chromeId).catch(() => 1);
    // Chromium doesn't tell extensions whether a tab can go back; the buttons stay enabled.
    navState = { canGoBack: true, canGoForward: true, zoom: Math.round(zoom * 100) };
  }

  let stateTimer: ReturnType<typeof setTimeout> | undefined;
  const sendStateSoon = () => {
    clearTimeout(stateTimer);
    stateTimer = setTimeout(sendState, 16);
  };
  // Icons for tabs that haven't loaded their own yet.
  const wantFavicons = () => void favicons.want(Object.values(store.state.tabs).filter((t) => !t.favicon).map((t) => t.url));
  store.onChange(() => {
    sync();
    sendStateSoon();
    wantFavicons();
  });

  // ---- Chromium → Flecha ----------------------------------------------------

  async function onCreated(tab: chrome.tabs.Tab) {
    if (creating || flechaIdFor(tab.id!) || tab.id === placeholder || isShellPage(tab)) return;
    // A Flechinha's page isn't one of Flecha's tabs. Asked of the browser: a new Flechinha's
    // first tab arrives before openFlechinha() has told us its window.
    if (await isFlechinhaWindow(tab.windowId)) return;
    const opener = tab.openerTabId !== undefined ? flechaIdFor(tab.openerTabId) : undefined;
    const added = addTab(url(tab), { activate: tab.active, afterTabId: opener });
    live.set(added.id, tab.id!);
    rememberLive();
  }

  function onUpdated(chromeId: number, info: chrome.tabs.OnUpdatedInfo, tab: chrome.tabs.Tab) {
    const id = flechaIdFor(chromeId);
    if (!id) return;
    const patch: Parameters<Store['updateTab']>[1] = {};
    // Chromium's new tab page is Flecha's empty state: the tab is still a blank one.
    if (info.url) patch.url = isStartPage({ url: info.url } as chrome.tabs.Tab) ? 'about:blank' : info.url;
    // A blank tab keeps its "New Tab" name: the new tab page's own title and icon aren't its.
    const blank = (patch.url ?? store.state.tabs[id]?.url) === 'about:blank';
    if (info.title && !blank) patch.title = info.title;
    if (blank) patch.title = 'New Tab';
    if (info.favIconUrl && !blank) patch.favicon = info.favIconUrl;
    if (blank) patch.favicon = undefined;
    if (info.status) patch.loading = info.status === 'loading';
    if (info.audible !== undefined) patch.audible = info.audible;
    if (info.mutedInfo) patch.muted = info.mutedInfo.muted;
    store.updateTab(id, patch);
    if (info.status === 'complete' && tab.active) void refreshNav().then(sendState);
    if (info.splitViewId !== undefined) void splitChanged(chromeId, info.splitViewId);
  }

  function onActivated({ tabId }: { tabId: number }) {
    const id = flechaIdFor(tabId);
    if (id && store.activeSpace.activeTabId !== id) store.activate(id);
  }

  function onRemoved(chromeId: number) {
    if (ours.delete(chromeId)) return;
    if (chromeId === placeholder) placeholder = undefined;
    const id = flechaIdFor(chromeId);
    if (!id) return;
    live.delete(id);
    rememberLive();
    store.closeTab(id);
  }

  // ---- commands from Flecha pages -------------------------------------------

  const activeChromeId = () => (store.activeTab ? live.get(store.activeTab.id) : undefined);
  const onActive = (fn: (chromeId: number) => unknown) => () => {
    const chromeId = activeChromeId();
    if (chromeId !== undefined) void fn(chromeId);
  };
  const openTab = (input: string, activate = true) =>
    addTab(resolveInput(input, store.state.searchEngine), {
      activate,
      afterTabId: activate ? undefined : store.activeSpace.activeTabId,
    });

  // ---- context menus, as the Electron spike's tabMenu / spaceMenu ----------------
  // Built here, where the Store is; the page that asked draws them (windowUi.ts).

  const SEPARATOR = { separator: true } as const;
  const does = (name: string, ...args: unknown[]): MenuRun => ({ cmd: name, args });
  const spaceLabel = (s: { emoji?: string; name: string }) => `${s.emoji ? s.emoji + '  ' : ''}${s.name}`;

  function tabMenu(tabId: string): MenuEntry<MenuRun>[] | undefined {
    const tab = store.state.tabs[tabId];
    const zone = store.locate(tabId)?.zone;
    if (!tab || !zone) return undefined;
    const space = store.activeSpace;
    const split: MenuEntry<MenuRun>[] = [];
    if (space.split?.includes(tabId)) split.push({ label: 'Exit Split View', run: does('exitSplit') });
    else if (space.activeTabId && space.activeTabId !== tabId) split.push({ label: 'Open in Split View', run: does('splitWith', tabId) });
    const name = tab.customTitle ?? tab.title;
    const otherSpaces = store.state.spaces.filter((s) => s.id !== store.locate(tabId)?.space?.id);
    return [
      ...split,
      ...(split.length ? [SEPARATOR] : []),
      { label: 'Copy Link', run: { copy: tab.url, message: 'Link copied' } },
      { label: 'Copy as Markdown Link', run: { copy: `[${name}](${tab.url})`, message: 'Markdown link copied' } },
      { label: 'Rename…', run: { rename: tabId } },
      { label: 'Duplicate', run: does('newTab', tab.url) },
      // A Flechinha window of its own; the window code lives in another shell module.
      { label: 'Open in Flechinha', run: does('openFlechinha', tab.url, tabId) },
      SEPARATOR,
      { label: zone === 'today' ? 'Pin Tab' : 'Unpin Tab', run: does('togglePin', tabId) },
      { label: zone === 'favorites' ? 'Remove from Favorites' : 'Add to Favorites', run: does('toggleFavorite', tabId) },
      {
        label: 'Move to Space',
        enabled: zone !== 'favorites' && otherSpaces.length > 0,
        submenu: otherSpaces.map((s) => ({ label: spaceLabel(s), run: does('moveToSpace', tabId, s.id) })),
      },
      SEPARATOR,
      { label: 'Reload', enabled: live.has(tabId), run: does('reloadTab', tabId) },
      { label: tab.muted ? 'Unmute Tab' : 'Mute Tab', run: does('toggleMute', tabId) },
      ...(tab.homeUrl && tab.homeUrl !== tab.url ? [{ label: 'Go Back to Pinned URL', run: does('goHome', tabId) }] : []),
      SEPARATOR,
      { label: zone === 'today' ? 'Close Tab' : 'Unload Tab', run: does('close', tabId) },
      ...(zone === 'today'
        ? [
            { label: 'Close Tabs Below', run: does('closeTabs', tabId, 'below') },
            { label: 'Close Other Tabs', run: does('closeTabs', tabId, 'others') },
          ]
        : []),
    ];
  }

  function spaceMenu(spaceId: string): MenuEntry<MenuRun>[] | undefined {
    const space = store.state.spaces.find((s) => s.id === spaceId);
    if (!space) return undefined;
    return [
      { label: 'Edit Space…', run: does('editSpace', spaceId) },
      { label: 'New Folder', run: does('newFolder') },
      SEPARATOR,
      { label: 'Separate Cookies and Logins', checked: !!space.isolated, run: does('setSpaceIsolated', spaceId, !space.isolated) },
      SEPARATOR,
      { label: 'Delete Space…', enabled: store.state.spaces.length > 1, run: { deleteSpace: spaceId } },
    ];
  }

  /** Opens the sidebar's Space editor (name, emoji, colour), in that Space. */
  function editSpace(spaceId: string) {
    if (store.state.activeSpaceId !== spaceId) store.switchSpace(spaceId);
    broadcast('edit-space', spaceId);
  }

  function toggleMute(tabId: string) {
    const tab = store.state.tabs[tabId];
    if (!tab) return;
    const chromeId = live.get(tabId);
    // An unloaded tab remembers it, and opens muted.
    if (chromeId === undefined) store.updateTab(tabId, { muted: !tab.muted });
    else void chrome.tabs.update(chromeId, { muted: !tab.muted });
  }

  handle({
    ready: sendState,
    newTab: (input: string) => openTab(input),
    navigate: (input: string) => {
      const chromeId = activeChromeId();
      if (chromeId === undefined) return openTab(input);
      void chrome.tabs.update(chromeId, { url: resolveInput(input, store.state.searchEngine) });
    },
    activate: (id: string) => store.activate(id),
    close: (id: string) => {
      store.closeTab(id);
      void unload([id]);
    },
    back: onActive((id) => chrome.tabs.goBack(id).catch(() => undefined)),
    forward: onActive((id) => chrome.tabs.goForward(id).catch(() => undefined)),
    reload: onActive((id) => chrome.tabs.reload(id)),
    switchSpace: (id: string) => store.switchSpace(id),
    newSpace: () => editSpace(store.newSpace().id),
    editSpace: (id: string) => editSpace(String(id)),
    newFolder: () => store.newFolder(),
    tabMenu: (id: string) => tabMenu(String(id)),
    spaceMenu: (id: string) => spaceMenu(String(id)),
    setOverlay: (name: string, on: boolean) => {
      if (on) frame.overlays.add(String(name));
      else frame.overlays.delete(String(name));
      sendFrame();
    },
    togglePin: (id: string) => store.togglePin(id),
    toggleFavorite: (id: string) => store.toggleFavorite(id),
    moveToSpace: (id: string, spaceId: string) => store.moveToSpace(id, spaceId),
    reloadTab: (id: string) => {
      const chromeId = live.get(id);
      if (chromeId !== undefined) void chrome.tabs.reload(chromeId);
    },
    goHome: (id: string) => {
      const home = store.state.tabs[id]?.homeUrl;
      const chromeId = live.get(id);
      if (!home) return;
      if (chromeId !== undefined) void chrome.tabs.update(chromeId, { url: home });
      else store.updateTab(id, { url: home });
    },
    closeTabs: (id: string, which: string) => void unload(store.closeTodayTabs(id, which === 'others' ? 'others' : 'below')),
    updateSpace: (id: string, patch: { name?: string; emoji?: string; color?: string }) => store.updateSpace(id, patch ?? {}),
    // Spaces with their own cookies need a separate Chromium profile; not in this build yet.
    setSpaceIsolated: () => broadcast('flash', 'Separate cookies aren’t in this build yet'),
    deleteSpace: (id: string) => {
      if (store.state.spaces.length > 1) void unload(store.deleteSpace(id));
    },
    toggleFolder: (id: string) => store.toggleFolder(id),
    renameFolder: (id: string, name: string) => store.renameFolder(id, name),
    moveTab: (id: string, target: DropTarget) => store.moveTab(id, target),
    clearToday: () => void unload(store.clearToday()),
    renameTab: (id: string, title: string) => store.updateTab(id, { customTitle: String(title).trim() || undefined }),
    toggleMute: (id: string) => toggleMute(String(id)),
    flash: (message: string) => broadcast('flash', String(message)),
    openPalette: (mode: string) => openPalette(mode),
    closePalette: () => {
      frame.paletteOpen = false;
      sendFrame();
      if (hasFrame()) void chrome.flechaPrivate.focus('page');
    },
    toggleSidebar: () => {
      if (tourKey('toggleSidebar')) return;
      frame.sidebarPeek = false;
      store.setSidebarVisible(!store.state.sidebarVisible);
    },
    setSidebarPeek: (peek: boolean) => {
      if (store.state.sidebarVisible || frame.sidebarPeek === !!peek) return;
      frame.sidebarPeek = !!peek;
      sendState();
    },
    setTabDragging: (dragging: boolean) => {
      frame.tabDragging = !!dragging;
      sendFrame();
    },
    zoom: (step: number) => {
      const chromeId = activeChromeId();
      if (chromeId === undefined) return;
      void chrome.tabs.getZoom(chromeId).then((zoom) => {
        const next = step === 0 ? 1 : Math.min(5, Math.max(0.25, zoom + (step > 0 ? 0.1 : -0.1)));
        return chrome.tabs.setZoom(chromeId, next).then(() => refreshNav().then(sendState));
      });
    },
    splitWith: (id: string) => store.splitWith(id),
    exitSplit,
    dropToSplit: (id: string, side: string) => dropToSplit(String(id), side === 'left' ? 'left' : 'right'),
    splitNew: (input: string) => store.splitWith(openTab(input, false).id),
    openSettings: (pane?: string) => openSettings(typeof pane === 'string' ? pane : undefined),
    finishOnboarding: () => store.setOnboarded(true),
    setPreferences: (prefs: { theme?: string; searchEngine?: string; adBlock?: boolean }) => {
      if (prefs?.theme === 'system' || prefs?.theme === 'light' || prefs?.theme === 'dark') store.setPreferences({ theme: prefs.theme });
      if (prefs?.searchEngine) store.setSearchEngine(prefs.searchEngine as SearchEngineId);
      if (typeof prefs?.adBlock === 'boolean') store.setAdBlock(prefs.adBlock);
    },
    searchHistory: async (query: string) =>
      (await chrome.history.search({ text: String(query), maxResults: 50, startTime: 0 })).filter((h) => isWebPage(h.url)).map((h) => ({
        url: h.url ?? '',
        title: h.title || h.url || '',
        visits: h.visitCount ?? 0,
        lastVisit: h.lastVisitTime ?? 0,
      })),
    topSites: async () => {
      const sites = (await chrome.topSites.get()).filter((s) => isWebPage(s.url));
      void favicons.want(sites.map((s) => s.url));
      return sites.map((s) => ({ url: s.url, title: s.title || s.url, visits: 0, lastVisit: 0 }));
    },
    importFlechaState: (json: string) => {
      let state: BrowserState | undefined;
      try {
        state = JSON.parse(String(json)) as BrowserState;
      } catch {
        // Reported below.
      }
      // The tab ids change, so every open Chromium tab is closed and reopens on demand.
      const open = [...live.keys()];
      if (!state || !store.replaceState(state)) return broadcast('flash', 'That isn’t a Flecha state file');
      void unload(open);
      const tabs = Object.keys(state.tabs).length;
      broadcast('flash', `Imported ${state.spaces.length} Spaces and ${tabs} tabs`);
    },
  });

  registerSiteControls({ store, activeChromeId, mainWindow });

  // ---- commands by id, for keyboard shortcuts (the Electron spike's buildMenu) ----

  /** Copies through the window's frame page: the background has no document for the clipboard. */
  async function copyToClipboard(text: string, message: string) {
    broadcast('clipboard', await mainWindow(), { text, message });
  }

  /** ⌘1–⌘8 pick a tab by position (favorites, then pinned, then today); ⌘9 the last one. */
  function selectTabIndex(index: number) {
    const ids = store.allTabIds(store.activeSpace);
    const id = index < 0 ? ids[ids.length - 1] : ids[index];
    if (id) store.activate(id);
  }

  function cycleTab(step: 1 | -1) {
    const space = store.activeSpace;
    const ids = store.allTabIds(space);
    if (!ids.length) return;
    const i = space.activeTabId ? ids.indexOf(space.activeTabId) : -1;
    store.activate(ids[(i + step + ids.length) % ids.length]);
  }

  function cycleSpace(step: 1 | -1) {
    const spaces = store.state.spaces;
    const i = spaces.findIndex((sp) => sp.id === store.state.activeSpaceId);
    store.switchSpace(spaces[(i + step + spaces.length) % spaces.length].id);
  }

  /** A command on the active tab, if there is one. */
  const withTab = (fn: (tab: NonNullable<Store['activeTab']>) => unknown) => () => {
    const tab = store.activeTab;
    if (tab) return fn(tab);
  };

  const commands: Record<ShortcutId | 'closePeek', () => unknown> = {
    newTab: () => openPalette('new-tab'),
    openLocation: () => openPalette('navigate'),
    back: () => call('back'),
    forward: () => call('forward'),
    reload: () => call('reload'),
    hardReload: onActive((id) => chrome.tabs.reload(id, { bypassCache: true })),
    // TODO(chromium): Chromium's own find bar does find in page; when a native patch routes ⌘F
    // here instead, whatever module draws Flecha's find bar handles this command.
    find: () => call('find'),
    toggleSidebar: () => call('toggleSidebar'),
    settings: () => openSettings(),
    newFlechinha: () => call('newFlechinha'),
    closeTab: withTab((tab) => call('close', tab.id)),
    reopenTab: () => {
      const item = store.popArchive();
      if (item) openTab(item.url);
    },
    nextTab: () => cycleTab(1),
    previousTab: () => cycleTab(-1),
    pinTab: withTab((tab) => store.togglePin(tab.id)),
    duplicateTab: withTab((tab) => openTab(tab.url)),
    muteTab: withTab((tab) => toggleMute(tab.id)),
    clearToday: () => call('clearToday'),
    copyLink: withTab((tab) => copyToClipboard(tab.url, 'Link copied')),
    copyMarkdown: withTab((tab) => copyToClipboard(`[${tab.customTitle ?? tab.title}](${tab.url})`, 'Markdown link copied')),
    screenshot: () => call('capturePage'),
    nextSpace: () => cycleSpace(1),
    previousSpace: () => cycleSpace(-1),
    newSpace: () => call('newSpace'),
    splitNew: () => openPalette('split'),
    exitSplit,
    zoomIn: () => call('zoom', 1),
    zoomOut: () => call('zoom', -1),
    actualSize: () => call('zoom', 0),
    // Owned by other shell modules, which handle a hub command of the same name.
    devTools: () => call('devTools'),
    responsive: () => call('responsive'),
    nextEnvironment: () => call('nextEnvironment'),
    editBoost: () => call('editBoost'),
    closePeek: () => call('closePeek'),
    viewSource: withTab((tab) => isWebPage(tab.url) && addTab(`view-source:${tab.url}`, { activate: true })),
    shortcuts: () => openSettings('shortcuts'),
  };

  /**
   * Runs a command by its id in src/main/shortcuts.ts, plus "tab1"…"tab9" (⌘1…⌘9, 9 is the
   * last tab) and "space1"…"space9" (⌃1…⌃9). "tabN" / "spaceN" take the number as `n`.
   * Returns whether the id is a command.
   */
  function runCommand(id: string, n?: number): boolean {
    const numbered = /^(tab|space)(\d|N)$/.exec(id);
    if (numbered) {
      const i = numbered[2] === 'N' ? Number(n) : Number(numbered[2]);
      if (!Number.isInteger(i) || i < 1 || i > 9) return false;
      if (numbered[1] === 'tab') selectTabIndex(i === 9 ? -1 : i - 1);
      else if (store.state.spaces[i - 1]) store.switchSpace(store.state.spaces[i - 1].id);
      return true;
    }
    if (!Object.hasOwn(commands, id)) return false;
    void Promise.resolve(commands[id as keyof typeof commands]()).catch((err) => console.error(`Flecha shell: "${id}" failed`, err));
    return true;
  }
  handle({ run: (id: string, n?: number) => runCommand(String(id), n) });

  registerSettings({
    store,
    unload: (ids) => void unload(ids),
    broadcastState: sendState,
    isOpen: (id) => live.has(id),
    load: (id, url) => {
      const chromeId = live.get(id);
      if (chromeId !== undefined) void chrome.tabs.update(chromeId, { url });
    },
    agentEndpoint: () => nativeHost?.agentEndpoint(),
    defaultBrowser: () => defaultBrowser,
    makeDefaultBrowser: async () => {
      await call('makeDefaultBrowser');
      await refreshDefaultBrowser();
    },
  });
  void refreshDefaultBrowser();
  chrome.windows.onFocusChanged.addListener((id) => id !== chrome.windows.WINDOW_ID_NONE && void refreshDefaultBrowser());

  // Boosts, environments, local servers, the responsive view, console errors and ad blocking.
  features = registerFeatures({ store, broadcastState: sendState, liveTabId: (id) => live.get(id), activeChromeTabId: activeChromeId });

  // Imports and the coding-agent bridge, through Flecha's native messaging host.
  nativeHost = registerNativeHost({
    store,
    broadcastState: sendState,
    tabs: {
      open: (url, background) => openTab(url, !background).id,
      chromeTab: async (id) => {
        await syncing;
        return live.get(id) ?? (store.state.tabs[id] ? openChromeTab(id, false) : undefined);
      },
      activate: (id) => store.activate(id),
      close: (id) => {
        store.closeTab(id);
        void unload([id]);
      },
    },
  });

  onConnect(() => {
    sendState();
    void siteIcons.all().then((icons) => broadcast('site-icons', icons));
  });

  // Flecha's keyboard shortcuts, taken by the browser before the page (patch 0016).
  registerNative({
    store,
    chromeTabId: (id) => live.get(id),
    flechaTabId: (chromeId) => flechaIdFor(chromeId),
    run: (command, windowId) => {
      // In a Flechinha, its own toolbar answers the few shortcuts it has.
      if (isFlechinha(windowId)) return broadcast('flechinha:command', windowId, command);
      const numbered = /^select(Tab|Space):(\d)$/.exec(command);
      runCommand(numbered ? `${numbered[1].toLowerCase()}${numbered[2]}` : command);
    },
    // Into a Flechinha: the page leaves its Space without closing.
    releaseTab: (id) => {
      live.delete(id);
      rememberLive();
      store.closeTab(id);
    },
    // Back from a Flechinha ("Open in Flecha"): an active Today tab in the current Space.
    adoptTab: (chromeId) => {
      void chrome.tabs.get(chromeId).then((tab) => {
        if (flechaIdFor(chromeId)) return;
        live.set(addTab(url(tab), { activate: true }).id, chromeId);
        rememberLive();
      });
    },
  });
  sync();
  wantFavicons();
  // Arc's auto-archive, hourly as in Electron. An alarm, since the background sleeps.
  // Made once: making it again on every wake would restart the hour.
  void chrome.alarms.get(ARCHIVE_ALARM).then((alarm) => void (alarm ?? chrome.alarms.create(ARCHIVE_ALARM, { periodInMinutes: 60 })));
  const archiveStale = () => void unload(store.archiveStale());

  return { onCreated, onUpdated, onActivated, onRemoved, openPalette, runCommand, archiveStale };
}

const url = (tab: chrome.tabs.Tab) => tab.pendingUrl || tab.url || '';
/** Flecha's own pages (the sidebar opened in a tab while debugging, say) aren't browsing tabs. */
const isShellPage = (tab: chrome.tabs.Tab) => url(tab).startsWith(chrome.runtime.getURL(''));
/** Sites worth suggesting: not Flecha's or Chromium's own pages. */
const isWebPage = (address = '') => /^https?:/.test(address);
const isStartPage = (tab: chrome.tabs.Tab) => /^chrome:\/\/(newtab|new-tab-page)\/?$/.test(url(tab));

// Listeners go on in the first turn, or Chromium drops the event that woke the background;
// each waits for the saved state to load.
const ready = start();
listen(ready);

/**
 * Runs a Flecha command by id (src/main/shortcuts.ts, plus tab1…tab9 and space1…space9), as
 * the Electron spike's menu accelerators did: for keyboard shortcuts from the browser
 * (chrome.flechaPrivate.onShortcut). Resolves to whether the id is a command. The same is the
 * hub command "run", for pages.
 */
export function run(id: string, n?: number): Promise<boolean> {
  return ready.then((r) => r.runCommand(id, n));
}
registerDownloads();
chrome.tabs.onCreated.addListener((tab) => void ready.then((r) => r.onCreated(tab)));
chrome.tabs.onUpdated.addListener((id, info, tab) => void ready.then((r) => r.onUpdated(id, info, tab)));
chrome.tabs.onActivated.addListener((info) => void ready.then((r) => r.onActivated(info)));
chrome.tabs.onRemoved.addListener((id) => void ready.then((r) => r.onRemoved(id)));
chrome.alarms.onAlarm.addListener((alarm) => alarm.name === ARCHIVE_ALARM && void ready.then((r) => r.archiveStale()));

// Until the native patch opens it on its own, the toolbar button opens the sidebar.
void chrome.sidePanel.setPanelBehavior({ openPanelOnActionClick: true });

// ⌘L: the browser sends this instead of focusing its hidden address bar (patch 0009).
chrome.commands.onCommand.addListener((command) => {
  if (command === 'flecha-open-location') void ready.then((r) => r.openPalette('navigate'));
});
