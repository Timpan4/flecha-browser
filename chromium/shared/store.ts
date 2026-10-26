import type {
  BrowserState,
  ClosedWindow,
  DropTarget,
  Environment,
  FlechaWindow,
  PinnedNode,
  SavedSpace,
  SearchEngineId,
  Space,
  TabInfo,
  TabPlacement,
  WindowBounds,
  WindowSpace,
} from './types';
import type { ArcImport, ImportNode } from './importApply';
import { STATE_VERSION } from './savedState';
import { clampSidebarWidth } from './sidebarWidth';

const SPACE_COLORS = ['#cdd9f7', '#f6d6cc', '#d3ecd5', '#e8d6f3', '#f5e7c4', '#cfe8ec'];
const SPACE_EMOJIS = ['🏠', '💼', '🎨', '📚', '🧪', '🎮'];
const ARCHIVE_LIMIT = 500;
/** Closed windows kept for reopening: as many entries as Chromium's tab restore keeps (TabRestoreServiceHelper::kMaxEntries). */
const CLOSED_WINDOW_LIMIT = 25;

/** Where a tab is: a favorite, pinned in a Space, or a Today tab of a Space in one window. */
type Location = { space?: SavedSpace; zone: 'today' | 'pinned' | 'favorites'; windowId?: string };

/** Where the state lives: a file in Electron, extension storage in the Chromium build. */
export interface StatePersistence {
  load(): BrowserState | undefined;
  save(json: string): void | Promise<void>;
}

const randomUUID = () => crypto.randomUUID();

/**
 * What a change touched: 'tab' is only what one tab shows (its title, icon, address, loading or
 * sound), which never changes which tabs are open or on screen; 'structure' is everything else.
 */
export type StoreChange = 'structure' | 'tab';

/**
 * Flecha's Spaces, tabs and windows. Spaces, their pinned tabs and the favorites are shared by
 * every window; each window has its own active Space and, per Space, its own Today tabs, tab on
 * screen and split (Arc's window-local temporary tabs). Methods about one window take its id and
 * default to the focused window: the one keyboard shortcuts, Settings and the agent act in.
 */
export class Store {
  state: BrowserState;
  private saveTimer?: ReturnType<typeof setTimeout>;
  private listeners: Array<(change: StoreChange) => void> = [];

  constructor(private persistence: StatePersistence) {
    this.state = this.load() ?? this.initialState();
    if (!this.state.windows.length) this.state.windows.push(this.newWindow());
  }

  onChange(fn: (change: StoreChange) => void) {
    this.listeners.push(fn);
  }

  // ---- windows ----------------------------------------------------------------

  /** The window shortcuts, Settings and the agent act in: the most recently focused one. */
  get focusedWindowId(): string {
    return this.state.windows[0].id;
  }

  /** A window by id, or the focused one. */
  window(windowId?: string): FlechaWindow {
    return (windowId !== undefined && this.state.windows.find((w) => w.id === windowId)) || this.state.windows[0];
  }

  /** Whether a window with this id is open. */
  hasWindow(windowId: string) {
    return this.state.windows.some((w) => w.id === windowId);
  }

  /** A new window, showing `spaceId` or the focused window's Space. It is not focused. */
  openWindow(spaceId?: string): FlechaWindow {
    const win = this.newWindow(spaceId && this.savedSpace(spaceId) ? spaceId : this.window().activeSpaceId);
    this.state.windows.push(win);
    this.changed();
    return win;
  }

  /** Makes a window the focused one. */
  focusWindow(windowId: string) {
    const i = this.state.windows.findIndex((w) => w.id === windowId);
    if (i <= 0) return;
    this.state.windows.unshift(...this.state.windows.splice(i, 1));
    this.changed();
  }

  /**
   * A window closed: it goes to the closed windows with its Today tabs, to be reopened. The last
   * open window stays, so the next start shows it again (closing it quits the browser).
   * Returns the ids of tabs dropped for good because too many windows were closed.
   */
  closeWindow(windowId: string): string[] {
    const i = this.state.windows.findIndex((w) => w.id === windowId);
    if (i < 0 || this.state.windows.length === 1) return [];
    const [win] = this.state.windows.splice(i, 1);
    // Pinned tabs and favorites it showed stay in the other windows' sidebars; only its Today tabs go with it.
    const closed: ClosedWindow = { ...win, spaces: {}, closedAt: Date.now() };
    // It comes back on a Today tab of its own: the one it showed, or the one used last.
    const recent = (ids: string[]) => [...ids].sort((a, b) => (this.state.tabs[b]?.lastActive ?? 0) - (this.state.tabs[a]?.lastActive ?? 0))[0];
    for (const [spaceId, ws] of Object.entries(win.spaces)) {
      if (ws.today.length) closed.spaces[spaceId] = { today: ws.today, activeTabId: ws.today.includes(ws.activeTabId!) ? ws.activeTabId : recent(ws.today) };
    }
    const dropped: string[] = [];
    if (Object.keys(closed.spaces).length) {
      this.state.closedWindows.unshift(closed);
      for (const old of this.state.closedWindows.splice(CLOSED_WINDOW_LIMIT)) {
        for (const ws of Object.values(old.spaces)) for (const id of ws.today) dropped.push(id);
      }
      for (const id of dropped) {
        this.archive(this.state.tabs[id]);
        delete this.state.tabs[id];
      }
    }
    this.changed();
    return dropped;
  }

  /** When the most recently closed window was closed, if any is left to reopen. */
  lastClosedWindowAt(): number | undefined {
    return this.state.closedWindows[0]?.closedAt;
  }

  /** Opens the most recently closed window again, with its Today tabs, focused. */
  reopenWindow(): FlechaWindow | undefined {
    const closed = this.state.closedWindows.shift();
    if (!closed) return undefined;
    const { closedAt: _closedAt, ...win } = closed;
    // Its Spaces may have been deleted since.
    for (const spaceId of Object.keys(win.spaces)) if (!this.savedSpace(spaceId)) delete win.spaces[spaceId];
    if (!this.savedSpace(win.activeSpaceId)) win.activeSpaceId = this.state.spaces[0].id;
    this.state.windows.unshift(win);
    this.changed();
    return win;
  }

  /** A Space as `windowId` shows it. */
  view(space: SavedSpace, windowId?: string): Space {
    return { ...space, ...this.windowSpace(space.id, windowId) };
  }

  /** Every Space as one window shows it, and its active Space: what that window's pages draw. */
  windowView(windowId?: string): { windowId: string; activeSpaceId: string; spaces: Space[] } {
    const win = this.window(windowId);
    return { windowId: win.id, activeSpaceId: this.activeSpaceIn(win.id).id, spaces: this.state.spaces.map((s) => this.view(s, win.id)) };
  }

  // ---- what a window shows --------------------------------------------------------

  /** The focused window's active Space. */
  get activeSpace(): Space {
    return this.activeSpaceIn();
  }

  activeSpaceIn(windowId?: string): Space {
    const win = this.window(windowId);
    return this.view(this.savedSpace(win.activeSpaceId) ?? this.state.spaces[0], win.id);
  }

  /** The focused window's active tab. */
  get activeTab(): TabInfo | undefined {
    return this.activeTabIn();
  }

  activeTabIn(windowId?: string): TabInfo | undefined {
    const id = this.activeSpaceIn(windowId).activeTabId;
    return id ? this.state.tabs[id] : undefined;
  }

  /** Favorites, then the Space's pinned tabs, then its Today tabs in the window. */
  allTabIds(space: Pick<SavedSpace, 'id' | 'pinned'>, windowId?: string): string[] {
    return [...this.state.favorites, ...pinnedTabIds(space.pinned), ...this.windowSpace(space.id, windowId).today];
  }

  /** The tabs on screen in a window right now: the active tab, or both halves of its split. */
  visibleTabIds(windowId?: string): string[] {
    const space = this.activeSpaceIn(windowId);
    const active = space.activeTabId;
    if (!active) return [];
    return space.split?.includes(active) ? [...space.split] : [active];
  }

  /** The open window whose active Space has this tab on screen, if any. */
  windowShowing(tabId: string): string | undefined {
    return this.state.windows.find((w) => this.visibleTabIds(w.id).includes(tabId))?.id;
  }

  locate(tabId: string): Location | undefined {
    if (this.state.favorites.includes(tabId)) return { zone: 'favorites' };
    for (const space of this.state.spaces) if (pinnedTabIds(space.pinned).includes(tabId)) return { space, zone: 'pinned' };
    for (const win of this.state.windows) {
      for (const [spaceId, ws] of Object.entries(win.spaces)) {
        if (ws.today.includes(tabId)) return { space: this.savedSpace(spaceId), zone: 'today', windowId: win.id };
      }
    }
    return undefined;
  }

  // ---- tabs -----------------------------------------------------------------

  addTab(url: string, opts: { activate?: boolean; afterTabId?: string; windowId?: string } = {}): TabInfo {
    const tab: TabInfo = { id: randomUUID(), url, title: url, lastActive: Date.now() };
    this.state.tabs[tab.id] = tab;
    const win = this.window(opts.windowId);
    const ws = this.windowSpace(win.activeSpaceId, win.id);
    const after = opts.afterTabId ? ws.today.indexOf(opts.afterTabId) : -1;
    ws.today.splice(after + 1, 0, tab.id);
    if (opts.activate !== false) ws.activeTabId = tab.id;
    this.changed();
    return tab;
  }

  /**
   * Shows a tab. A Today tab shows in its own window; a pinned tab or favorite in the window that
   * asks (default: the focused one), leaving any other window that showed it. `recent: false`
   * passes through the tab without making it the most recently used. Returns the window it shows in.
   */
  activate(tabId: string, opts: { recent?: boolean; windowId?: string } = {}): string | undefined {
    const tab = this.state.tabs[tabId];
    const loc = this.locate(tabId);
    if (!tab || !loc) return undefined;
    const win = this.window(loc.windowId ?? opts.windowId);
    if (loc.space) win.activeSpaceId = loc.space.id;
    if (loc.zone !== 'today') this.claim(tabId, win);
    this.windowSpace(win.activeSpaceId, win.id).activeTabId = tabId;
    if (opts.recent !== false) tab.lastActive = Date.now();
    this.changed();
    return win.id;
  }

  /** Makes a tab the most recently used. */
  touch(tabId: string) {
    const tab = this.state.tabs[tabId];
    if (!tab) return;
    tab.lastActive = Date.now();
    this.changed();
  }

  updateTab(tabId: string, patch: Partial<TabInfo>) {
    const tab = this.state.tabs[tabId];
    if (!tab) return;
    const keys = Object.keys(patch) as Array<keyof TabInfo>;
    if (keys.every((k) => tab[k] === patch[k])) return;
    Object.assign(tab, patch);
    this.changed('tab');
  }

  /**
   * Today tabs are archived and removed. Pinned tabs and favorites stay in the
   * sidebar and only unload, going back to their home URL (Arc's behaviour).
   */
  closeTab(tabId: string): 'removed' | 'unloaded' {
    const tab = this.state.tabs[tabId];
    const loc = this.locate(tabId);
    if (!tab || !loc) return 'removed';

    if (loc.zone !== 'today') {
      if (tab.homeUrl) tab.url = tab.homeUrl;
      delete tab.history;
      for (const win of this.state.windows) for (const ws of Object.values(win.spaces)) if (ws.activeTabId === tabId) ws.activeTabId = undefined;
      this.changed();
      return 'unloaded';
    }

    const ws = this.windowSpace(loc.space!.id, loc.windowId);
    const index = ws.today.indexOf(tabId);
    ws.today.splice(index, 1);
    this.archive(tab);
    delete this.state.tabs[tabId];
    if (ws.activeTabId === tabId) ws.activeTabId = ws.today[Math.min(index, ws.today.length - 1)];
    this.changed();
    return 'removed';
  }

  /** Moves a tab between favorites, pinned (optionally into a folder) and today, in a window's active Space. */
  moveTab(tabId: string, target: DropTarget, windowId?: string) {
    const tab = this.state.tabs[tabId];
    const from = this.locate(tabId);
    if (!tab || !from || target.beforeTabId === tabId) return;
    const win = this.window(windowId);
    const targetSpace = this.savedSpace(win.activeSpaceId) ?? this.state.spaces[0];

    this.detachEverywhere(tabId);
    const insert = (list: string[]) => {
      const i = target.beforeTabId ? list.indexOf(target.beforeTabId) : -1;
      list.splice(i < 0 ? list.length : i, 0, tabId);
    };

    if (target.zone === 'favorites') {
      insert(this.state.favorites);
    } else if (target.zone === 'today') {
      insert(this.windowSpace(targetSpace.id, win.id).today);
    } else {
      const parent = target.folderId ? findFolder(targetSpace.pinned, target.folderId)?.children : undefined;
      const list = parent ?? (target.beforeTabId ? findParentList(targetSpace.pinned, target.beforeTabId) : undefined) ?? targetSpace.pinned;
      const i = target.beforeTabId ? list.findIndex((n) => n.kind === 'tab' && n.tabId === target.beforeTabId) : -1;
      list.splice(i < 0 ? list.length : i, 0, { kind: 'tab', tabId });
    }

    if (target.zone === 'today') delete tab.homeUrl;
    else tab.homeUrl ??= tab.url;
    this.forgetMissingActive();
    this.changed();
  }

  /**
   * Moves a tab anywhere, in any Space (Settings' tab manager); a Today tab goes to that Space in
   * `windowId`. Unlike `moveTab`, the target names its Space. Returns whether it moved.
   */
  placeTab(tabId: string, to: TabPlacement, windowId?: string): boolean {
    const tab = this.state.tabs[tabId];
    if (!tab || !this.locate(tabId) || to.beforeId === tabId || !this.placementExists(to)) return false;
    this.detachEverywhere(tabId);
    this.insert(tabId, to, windowId);
    if (to.zone === 'today') delete tab.homeUrl;
    else tab.homeUrl ??= tab.url;
    this.forgetMissingActive();
    this.changed();
    return true;
  }

  /** A new pinned tab, favorite or Today tab (of `windowId`) that isn't opened yet. */
  createTab(url: string, title: string, to: TabPlacement, windowId?: string): TabInfo | undefined {
    if (!this.placementExists(to)) return undefined;
    const tab: TabInfo = { id: randomUUID(), url, title: title || url, lastActive: Date.now() };
    if (to.zone !== 'today') tab.homeUrl = url;
    this.state.tabs[tab.id] = tab;
    this.insert(tab.id, to, windowId);
    this.changed();
    return tab;
  }

  /**
   * Deletes tabs for good, wherever they are: pinned tabs and favorites too, which
   * `closeTab` only unloads. They go to the archive, so they can still be found. Returns the removed ids.
   */
  removeTabs(tabIds: string[]): string[] {
    const removed = tabIds.filter((id) => this.state.tabs[id] && this.locate(id));
    for (const id of removed) {
      this.detachEverywhere(id);
      this.archive(this.state.tabs[id]);
      delete this.state.tabs[id];
    }
    if (!removed.length) return [];
    this.forgetMissingActive();
    this.changed();
    return removed;
  }

  // ---- split view -----------------------------------------------------------------

  /** Puts `tabId` next to the window's active tab, on the right unless `side` says otherwise. */
  splitWith(tabId: string, side: 'left' | 'right' = 'right', windowId?: string) {
    const win = this.window(windowId);
    const ws = this.windowSpace(win.activeSpaceId, win.id);
    const current = ws.activeTabId;
    const space = this.savedSpace(win.activeSpaceId);
    if (!space || !current || current === tabId || !this.allTabIds(space, win.id).includes(tabId)) return;
    // A pinned tab or favorite on screen elsewhere comes here, as when it's activated.
    this.claim(tabId, win);
    ws.split = side === 'left' ? [tabId, current] : [current, tabId];
    delete ws.splitMode;
    ws.activeTabId = tabId;
    this.state.tabs[tabId].lastActive = Date.now();
    this.changed();
  }

  /** Opens the active page again as a phone-sized mirror, side by side. Returns the mirror's id. */
  openResponsive(windowId?: string): string | undefined {
    const win = this.window(windowId);
    const ws = this.windowSpace(win.activeSpaceId, win.id);
    const source = this.activeTabIn(win.id);
    if (!source) return undefined;
    const mirror = this.addTab(source.url, { activate: false, afterTabId: ws.today.includes(source.id) ? source.id : undefined, windowId: win.id });
    mirror.device = 'mobile';
    mirror.title = source.title;
    ws.split = [source.id, mirror.id];
    ws.splitMode = 'responsive';
    this.changed();
    return mirror.id;
  }

  setSplitRatio(ratio: number, windowId?: string) {
    const win = this.window(windowId);
    const ws = this.windowSpace(win.activeSpaceId, win.id);
    if (!ws.split) return;
    ws.splitRatio = Math.min(0.8, Math.max(0.2, ratio));
    this.changed();
  }

  /** Ends the window's split. A responsive mirror has no life of its own, so its id is returned for closing. */
  unsplit(windowId?: string): string | undefined {
    const win = this.window(windowId);
    const ws = this.windowSpace(win.activeSpaceId, win.id);
    if (!ws.split) return undefined;
    const mirror = ws.splitMode === 'responsive' ? ws.split[1] : undefined;
    delete ws.split;
    delete ws.splitMode;
    this.changed();
    return mirror;
  }

  /** Moves a tab into another Space (pinned tabs keep being pinned there, Today tabs stay in their window). */
  moveToSpace(tabId: string, spaceId: string) {
    const from = this.locate(tabId);
    const to = this.savedSpace(spaceId);
    if (!from?.space || !to || from.space === to) return;
    const win = this.window(from.windowId);
    const fromWs = this.windowSpace(from.space.id, win.id);
    this.detachEverywhere(tabId);
    if (from.zone === 'pinned') to.pinned.push({ kind: 'tab', tabId });
    else this.windowSpace(to.id, win.id).today.unshift(tabId);
    if (fromWs.activeTabId === tabId) fromWs.activeTabId = fromWs.today[0];
    this.forgetMissingActive();
    this.changed();
  }

  /** Closes the today tabs after `tabId`, or all others when `others` is set. Returns their ids. */
  closeTodayTabs(tabId: string, which: 'below' | 'others'): string[] {
    const loc = this.locate(tabId);
    if (loc?.zone !== 'today' || !loc.space) return [];
    const today = this.windowSpace(loc.space.id, loc.windowId).today;
    const index = today.indexOf(tabId);
    const ids = today.filter((id, i) => (which === 'below' ? i > index : id !== tabId));
    for (const id of ids) this.closeTab(id);
    return ids;
  }

  togglePin(tabId: string, windowId?: string) {
    const loc = this.locate(tabId);
    this.moveTab(tabId, { zone: loc?.zone === 'today' ? 'pinned' : 'today' }, loc?.windowId ?? windowId);
  }

  toggleFavorite(tabId: string, windowId?: string) {
    const loc = this.locate(tabId);
    this.moveTab(tabId, { zone: loc?.zone === 'favorites' ? 'today' : 'favorites' }, loc?.windowId ?? windowId);
  }

  // ---- folders --------------------------------------------------------------------

  /** In the window's active Space unless `where` names another, optionally inside a folder. Returns the new folder's id. */
  newFolder(name = 'New Folder', where: { spaceId?: string; parentId?: string } = {}, windowId?: string): string | undefined {
    const space = this.savedSpace(where.spaceId ?? this.window(windowId).activeSpaceId);
    const list = where.parentId ? (space && findFolder(space.pinned, where.parentId))?.children : space?.pinned;
    if (!list) return undefined;
    const id = randomUUID();
    list.push({ kind: 'folder', id, name: name.trim().slice(0, 80) || 'New Folder', open: true, children: [] });
    this.changed();
    return id;
  }

  toggleFolder(folderId: string) {
    const folder = this.findFolder(folderId)?.folder;
    if (!folder) return;
    folder.open = !folder.open;
    this.changed();
  }

  renameFolder(folderId: string, name: string) {
    const folder = this.findFolder(folderId)?.folder;
    if (!folder || !name.trim()) return;
    folder.name = name.trim();
    this.changed();
  }

  /**
   * Removes a folder. Its tabs (and subfolders) take its place when `keepTabs` is set;
   * otherwise they are deleted too. Returns the ids of deleted tabs.
   */
  deleteFolder(folderId: string, keepTabs: boolean): string[] {
    const found = this.findFolder(folderId);
    if (!found) return [];
    const { folder, parent } = found;
    parent.splice(parent.indexOf(folder), 1, ...(keepTabs ? folder.children : []));
    const removed = keepTabs ? [] : pinnedTabIds(folder.children);
    for (const id of removed) {
      this.archive(this.state.tabs[id]);
      delete this.state.tabs[id];
    }
    this.forgetMissingActive();
    this.changed();
    return removed;
  }

  /** Moves a folder within the pinned tabs of any Space; never into itself. Returns whether it moved. */
  moveFolder(folderId: string, to: { spaceId: string; folderId?: string; beforeId?: string }): boolean {
    const found = this.findFolder(folderId);
    const space = this.savedSpace(to.spaceId);
    if (!found || !space || to.beforeId === folderId) return false;
    if (to.folderId && (to.folderId === folderId || findFolder(found.folder.children, to.folderId))) return false;
    const list = to.folderId ? findFolder(space.pinned, to.folderId)?.children : space.pinned;
    if (!list) return false;
    found.parent.splice(found.parent.indexOf(found.folder), 1);
    const i = to.beforeId ? list.findIndex((n) => nodeId(n) === to.beforeId) : -1;
    list.splice(i < 0 ? list.length : i, 0, found.folder);
    this.forgetMissingActive();
    this.changed();
    return true;
  }

  // ---- Spaces ---------------------------------------------------------------------

  /** Switches the window to the new Space unless `activate` is false (Settings adds one without leaving the current Space). */
  newSpace(activate = true, windowId?: string): Space {
    const n = this.state.spaces.length;
    const space: SavedSpace = {
      id: randomUUID(),
      name: `Space ${n + 1}`,
      emoji: SPACE_EMOJIS[n % SPACE_EMOJIS.length],
      color: SPACE_COLORS[n % SPACE_COLORS.length],
      pinned: [],
    };
    this.state.spaces.push(space);
    if (activate) this.window(windowId).activeSpaceId = space.id;
    this.changed();
    return this.view(space, windowId);
  }

  /** Reorders the Spaces: `spaceId` goes before `beforeSpaceId`, or last. */
  moveSpace(spaceId: string, beforeSpaceId?: string) {
    const space = this.savedSpace(spaceId);
    if (!space || beforeSpaceId === spaceId) return;
    const spaces = this.state.spaces.filter((s) => s !== space);
    const i = beforeSpaceId ? spaces.findIndex((s) => s.id === beforeSpaceId) : -1;
    spaces.splice(i < 0 ? spaces.length : i, 0, space);
    this.state.spaces = spaces;
    this.changed();
  }

  /** Name, emoji and colour. Invalid values are ignored rather than stored. */
  updateSpace(spaceId: string, patch: { name?: string; emoji?: string; color?: string }) {
    const space = this.savedSpace(spaceId);
    if (!space) return;
    const name = typeof patch.name === 'string' ? patch.name.trim().slice(0, 40) : '';
    if (name) space.name = name;
    if (typeof patch.emoji === 'string') space.emoji = [...patch.emoji.trim()].slice(0, 2).join('') || undefined;
    if (typeof patch.color === 'string' && /^#[0-9a-f]{6}$/i.test(patch.color)) space.color = patch.color.toLowerCase();
    this.changed();
  }

  /** Deletes a Space with its pinned tabs and every window's Today tabs in it. Returns the ids of tabs that were dropped, so their views can be destroyed. */
  deleteSpace(spaceId: string): string[] {
    if (this.state.spaces.length < 2) return [];
    const space = this.savedSpace(spaceId);
    if (!space) return [];
    const ids = [...pinnedTabIds(space.pinned)];
    for (const win of [...this.state.windows, ...this.state.closedWindows]) {
      ids.push(...(win.spaces[spaceId]?.today ?? []));
      delete win.spaces[spaceId];
    }
    for (const id of ids) delete this.state.tabs[id];
    this.state.spaces = this.state.spaces.filter((s) => s !== space);
    for (const win of [...this.state.windows, ...this.state.closedWindows]) if (win.activeSpaceId === spaceId) win.activeSpaceId = this.state.spaces[0].id;
    this.state.closedWindows = this.state.closedWindows.filter((w) => Object.keys(w.spaces).length);
    this.changed();
    return ids;
  }

  /** Returns the Space's tab ids in every open window: their pages must be reopened in the new session. */
  setIsolated(spaceId: string, isolated: boolean): string[] {
    const space = this.savedSpace(spaceId);
    if (!space) return [];
    space.isolated = isolated;
    this.changed();
    return [...pinnedTabIds(space.pinned), ...this.state.windows.flatMap((w) => w.spaces[spaceId]?.today ?? [])];
  }

  switchSpace(spaceId: string, windowId?: string) {
    if (!this.savedSpace(spaceId)) return;
    this.window(windowId).activeSpaceId = spaceId;
    this.changed();
  }

  /** Archives every Today tab in the window's active Space. Returns the removed ids. */
  clearToday(windowId?: string): string[] {
    const win = this.window(windowId);
    const ws = this.windowSpace(win.activeSpaceId, win.id);
    const ids = ws.today.splice(0);
    for (const id of ids) {
      this.archive(this.state.tabs[id]);
      delete this.state.tabs[id];
    }
    if (ws.activeTabId && ids.includes(ws.activeTabId)) ws.activeTabId = undefined;
    this.forgetMissingActive();
    this.changed();
    return ids;
  }

  /** Arc's auto-archive: Today tabs untouched for a while go away on their own, in every open window. */
  archiveStale(): string[] {
    const hours = this.state.archiveAfterHours ?? 12;
    if (hours === 0) return [];
    const cutoff = Date.now() - hours * 60 * 60 * 1000;
    const removed: string[] = [];
    for (const win of this.state.windows) {
      for (const ws of Object.values(win.spaces)) {
        ws.today = ws.today.filter((id) => {
          const tab = this.state.tabs[id];
          if (!tab || id === ws.activeTabId || tab.lastActive >= cutoff) return true;
          this.archive(tab);
          delete this.state.tabs[id];
          removed.push(id);
          return false;
        });
      }
    }
    if (removed.length) {
      this.forgetMissingActive();
      this.changed();
    }
    return removed;
  }

  popArchive() {
    const item = this.state.archive.shift();
    if (item) this.changed();
    return item;
  }

  // ---- preferences ----------------------------------------------------------------

  setSearchEngine(id: SearchEngineId) {
    this.state.searchEngine = id;
    this.changed();
  }

  setEnvironments(environments: Environment[]) {
    this.state.environments = environments;
    this.changed();
  }

  setDownloads(options: { folder?: string; askWhereToSave?: boolean }) {
    if (options.folder !== undefined) this.state.downloadsFolder = options.folder;
    if (options.askWhereToSave !== undefined) this.state.askWhereToSave = options.askWhereToSave;
    this.changed();
  }

  setAgent(agent: { enabled: boolean; token: string }) {
    this.state.agent = agent;
    this.changed();
  }

  sitePermission(origin: string, permission: string) {
    return this.state.sitePermissions?.[origin]?.[permission];
  }

  /** `undefined` forgets the decision, so the site asks again next time. */
  setSitePermission(origin: string, permission: string, decision: 'allow' | 'block' | undefined) {
    const all = (this.state.sitePermissions ??= {});
    const site = (all[origin] ??= {});
    if (decision) site[permission] = decision;
    else delete site[permission];
    if (!Object.keys(site).length) delete all[origin];
    this.changed();
  }

  setPreferences(prefs: { theme?: 'system' | 'light' | 'dark'; peekPinnedLinks?: boolean }) {
    if (prefs.theme && ['system', 'light', 'dark'].includes(prefs.theme)) this.state.theme = prefs.theme;
    if (typeof prefs.peekPinnedLinks === 'boolean') this.state.peekPinnedLinks = prefs.peekPinnedLinks;
    this.changed();
  }

  setOnboarded(done: boolean) {
    this.state.onboarded = done;
    this.changed();
  }

  setOpenLinksInFlechinha(enabled: boolean) {
    if (typeof enabled === 'boolean') this.state.openLinksInFlechinha = enabled;
    this.changed();
  }

  setArchiveAfter(hours: number) {
    this.state.archiveAfterHours = Math.max(0, hours);
    this.changed();
  }

  /** `keys`: an accelerator, null for none, or undefined to go back to the default. */
  setKeybinding(id: string, keys: string | null | undefined) {
    const all = (this.state.keybindings ??= {});
    if (keys === undefined) delete all[id];
    else all[id] = keys;
    this.changed();
  }

  resetKeybindings() {
    delete this.state.keybindings;
    this.changed();
  }

  setAdBlock(enabled: boolean) {
    this.state.adBlock = enabled;
    this.changed();
  }

  /** Saved without notifying listeners: nothing on screen depends on it. */
  setWindowBounds(bounds: WindowBounds) {
    this.state.window = bounds;
    this.scheduleSave();
  }

  /** Shows or hides one window's sidebar; windows opened later start the same way. */
  setSidebarVisible(visible: boolean, windowId?: string) {
    // The other windows keep what they showed.
    for (const win of this.state.windows) win.sidebarVisible ??= this.state.sidebarVisible;
    this.window(windowId).sidebarVisible = visible;
    this.state.sidebarVisible = visible;
    this.changed();
  }

  sidebarVisibleIn(windowId?: string): boolean {
    return this.window(windowId).sidebarVisible ?? this.state.sidebarVisible;
  }

  /** The sidebar's width, the same in every window. */
  setSidebarWidth(width: number) {
    this.state.sidebarWidth = clampSidebarWidth(width);
    this.changed();
  }

  // ---- imports --------------------------------------------------------------------

  /** Arc's Spaces become Flecha's; their Today tabs open in `windowId`. */
  importArc(data: ArcImport, windowId?: string) {
    const s = this.state;
    const pristine = this.dropPristineSpace();
    const win = this.window(windowId);

    for (const imported of data.spaces) {
      const n = s.spaces.length;
      const space: SavedSpace = {
        id: randomUUID(),
        name: imported.name,
        emoji: imported.emoji,
        color: SPACE_COLORS[n % SPACE_COLORS.length],
        pinned: this.importNodes(imported.pinned),
      };
      s.spaces.push(space);
      win.spaces[space.id] = { today: imported.today.map((t) => this.importTab(t, false)) };
    }
    for (const fav of data.favorites) s.favorites.push(this.importTab(fav, true));
    this.afterImport(pristine, s.spaces[0].id);
    this.changed();
  }

  /** Another browser's bookmarks become the pinned tabs and folders of a new Space named `name`. */
  importBookmarks(name: string, nodes: ImportNode[]): SavedSpace {
    const s = this.state;
    const pristine = this.dropPristineSpace();
    const n = s.spaces.length;
    const space: SavedSpace = {
      id: randomUUID(),
      name,
      emoji: SPACE_EMOJIS[n % SPACE_EMOJIS.length],
      color: SPACE_COLORS[n % SPACE_COLORS.length],
      pinned: this.importNodes(nodes),
    };
    s.spaces.push(space);
    this.afterImport(pristine, space.id);
    this.changed();
    return space;
  }

  /** Takes over another Flecha's whole state (its Spaces, tabs and settings), as if it had been loaded. */
  replaceState(state: BrowserState): boolean {
    if (!state?.spaces?.length || !state.tabs || !state.windows?.length) return false;
    this.state = state;
    this.changed();
    return true;
  }

  saveNow() {
    clearTimeout(this.saveTimer);
    const tabs = Object.fromEntries(
      Object.entries(this.state.tabs).map(([id, { loading, audible, ...tab }]) => [id, tab]),
    );
    return this.persistence.save(JSON.stringify({ version: STATE_VERSION, ...this.state, tabs }, null, 2));
  }

  // ---- internals ------------------------------------------------------------------

  /** A shared tab (pinned or favorite) shows in one window at a time: `win` takes it from the others. */
  private claim(tabId: string, win: FlechaWindow) {
    for (const other of this.state.windows) {
      if (other === win) continue;
      for (const ws of Object.values(other.spaces)) {
        if (ws.activeTabId === tabId) ws.activeTabId = undefined;
        if (ws.split?.includes(tabId)) {
          delete ws.split;
          delete ws.splitMode;
        }
      }
    }
  }

  private savedSpace(spaceId: string): SavedSpace | undefined {
    return this.state.spaces.find((s) => s.id === spaceId);
  }

  /** A Space's Today tabs and selection in a window (the focused one by default), made on first use. */
  private windowSpace(spaceId: string, windowId?: string): WindowSpace {
    const win = this.window(windowId);
    return (win.spaces[spaceId] ??= { today: [] });
  }

  private newWindow(spaceId = this.state?.spaces[0]?.id): FlechaWindow {
    return { id: randomUUID(), activeSpaceId: spaceId ?? '', spaces: {} };
  }

  private changed(change: StoreChange = 'structure') {
    for (const win of this.state.windows) {
      for (const space of this.state.spaces) {
        const ws = win.spaces[space.id];
        // A split ends as soon as either half leaves the Space.
        if (ws?.split && !ws.split.every((id) => this.allTabIds(space, win.id).includes(id))) {
          delete ws.split;
          delete ws.splitMode;
        }
      }
    }
    this.scheduleSave();
    for (const fn of this.listeners) fn(change);
  }

  private scheduleSave() {
    clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => this.saveNow(), 500);
  }

  /** A folder in any Space, with the list it sits in. */
  private findFolder(folderId: string) {
    for (const space of this.state.spaces) {
      const folder = findFolder(space.pinned, folderId);
      if (folder) return { space, folder, parent: findParentList(space.pinned, folderId)! };
    }
    return undefined;
  }

  private placementExists(to: TabPlacement) {
    if (to.zone === 'favorites') return true;
    const space = this.savedSpace(to.spaceId);
    if (!space) return false;
    return to.zone === 'today' || !to.folderId || !!findFolder(space.pinned, to.folderId);
  }

  /** Inserts a tab that is in no list at a placement `placementExists` accepted. */
  private insert(tabId: string, to: TabPlacement, windowId?: string) {
    const before = (list: string[]) => {
      const i = to.beforeId ? list.indexOf(to.beforeId) : -1;
      return i < 0 ? list.length : i;
    };
    if (to.zone === 'favorites') return void this.state.favorites.splice(before(this.state.favorites), 0, tabId);
    const space = this.savedSpace(to.spaceId)!;
    if (to.zone === 'today') {
      const today = this.windowSpace(space.id, windowId).today;
      return void today.splice(before(today), 0, tabId);
    }
    const list = to.folderId ? findFolder(space.pinned, to.folderId)!.children : space.pinned;
    const i = to.beforeId ? list.findIndex((n) => nodeId(n) === to.beforeId) : -1;
    list.splice(i < 0 ? list.length : i, 0, { kind: 'tab', tabId });
  }

  /** Takes a tab out of favorites, every Space and every window, editing the lists in place. */
  private detachEverywhere(tabId: string) {
    const drop = (list: string[]) => {
      const i = list.indexOf(tabId);
      if (i >= 0) list.splice(i, 1);
    };
    drop(this.state.favorites);
    for (const space of this.state.spaces) removePinned(space.pinned, tabId);
    for (const win of [...this.state.windows, ...this.state.closedWindows]) for (const ws of Object.values(win.spaces)) drop(ws.today);
  }

  /** A window's Space whose active tab went elsewhere shows nothing until you pick a tab. */
  private forgetMissingActive() {
    for (const win of this.state.windows) {
      for (const space of this.state.spaces) {
        const ws = win.spaces[space.id];
        if (ws?.activeTabId && !this.allTabIds(space, win.id).includes(ws.activeTabId)) ws.activeTabId = undefined;
      }
    }
  }

  /** A fresh install's single empty Space gives way to imported ones. Returns whether it did. */
  private dropPristineSpace() {
    const s = this.state;
    const only = s.spaces.length === 1 ? s.spaces[0] : undefined;
    const pristine = !!only && !pinnedTabIds(only.pinned).length && s.windows.every((w) => !w.spaces[only.id]?.today.length);
    if (pristine) {
      s.spaces = [];
      for (const win of s.windows) delete win.spaces[only!.id];
    }
    return pristine;
  }

  /** Windows whose Space went away (the pristine one), or every window after a fresh start, show `spaceId`. */
  private afterImport(pristine: boolean, spaceId: string) {
    for (const win of this.state.windows) if (pristine || !this.savedSpace(win.activeSpaceId)) win.activeSpaceId = spaceId;
  }

  private importTab(t: { url: string; title: string }, pinned: boolean) {
    const tab: TabInfo = { id: randomUUID(), url: t.url, title: t.title || t.url, lastActive: Date.now() };
    if (pinned) tab.homeUrl = t.url;
    this.state.tabs[tab.id] = tab;
    return tab.id;
  }

  private importNodes(nodes: ImportNode[]): PinnedNode[] {
    return nodes.map((n) =>
      n.kind === 'tab'
        ? { kind: 'tab', tabId: this.importTab(n, true) }
        : { kind: 'folder', id: randomUUID(), name: n.name, open: false, children: this.importNodes(n.children) },
    );
  }

  private archive(tab: TabInfo | undefined) {
    if (!tab || tab.url === 'about:blank') return;
    this.state.archive.unshift({ url: tab.url, title: tab.title, closedAt: Date.now() });
    this.state.archive.length = Math.min(this.state.archive.length, ARCHIVE_LIMIT);
  }

  private load(): BrowserState | undefined {
    const state = this.persistence.load();
    return state?.spaces?.length ? state : undefined;
  }

  private initialState(): BrowserState {
    const space: SavedSpace = { id: randomUUID(), name: 'Personal', emoji: '🏠', color: SPACE_COLORS[0], pinned: [] };
    return { spaces: [space], windows: [{ id: randomUUID(), activeSpaceId: space.id, spaces: {} }], closedWindows: [], favorites: [], tabs: {}, archive: [], sidebarVisible: true };
  }
}

export function pinnedTabIds(nodes: PinnedNode[]): string[] {
  return nodes.flatMap((n) => (n.kind === 'tab' ? [n.tabId] : pinnedTabIds(n.children)));
}

export function findFolder(nodes: PinnedNode[], id: string): Extract<PinnedNode, { kind: 'folder' }> | undefined {
  for (const n of nodes) {
    if (n.kind !== 'folder') continue;
    if (n.id === id) return n;
    const inner = findFolder(n.children, id);
    if (inner) return inner;
  }
  return undefined;
}

/** The list holding the tab or folder with this id. */
function findParentList(nodes: PinnedNode[], id: string): PinnedNode[] | undefined {
  for (const n of nodes) {
    if (nodeId(n) === id) return nodes;
    if (n.kind === 'folder') {
      const inner = findParentList(n.children, id);
      if (inner) return inner;
    }
  }
  return undefined;
}

function nodeId(node: PinnedNode) {
  return node.kind === 'tab' ? node.tabId : node.id;
}

function removePinned(nodes: PinnedNode[], tabId: string) {
  for (let i = nodes.length - 1; i >= 0; i--) {
    const n = nodes[i];
    if (n.kind === 'tab' && n.tabId === tabId) nodes.splice(i, 1);
    else if (n.kind === 'folder') removePinned(n.children, tabId);
  }
}
