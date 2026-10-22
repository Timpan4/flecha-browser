import path from 'node:path';
import {
  app,
  BaseWindow,
  screen,
  WebContentsView,
  Menu,
  MenuItemConstructorOptions,
  clipboard,
  dialog,
  ipcMain,
  session,
  type WebContents,
} from 'electron';
import type { DropTarget, PaletteMode, PeekState, TabInfo, UiState } from '../shared/types';
import { Store } from './store';
import { arcSidebarPath, readArcSidebar } from './arcImport';
import { resolveInput } from './url';
import { setAdBlocking } from './adblock';
import { History } from './history';
import { Downloads } from './downloads';
import { Settings } from './settings';
import { WEB_STORE_URL, installedExtensions, removeExtension, setupExtensions, type TabHost } from './extensions';
import type { ElectronChromeExtensions } from 'electron-chrome-extensions';

const SIDEBAR_WIDTH = 260;
const FIND_BAR = { width: 360, height: 40 };
const GAP = 8;
const ARCHIVE_AFTER_MS = 12 * 60 * 60 * 1000;
/** Back/forward entries kept per tab across restarts. */
const SAVED_HISTORY = 25;

app.setName('Flecha');
// Lets development runs use a throwaway profile: FLECHA_USER_DATA=/tmp/flecha npm start
if (process.env.FLECHA_USER_DATA) app.setPath('userData', process.env.FLECHA_USER_DATA);
// Some sites (Google sign-in in particular) refuse user agents that mention Electron.
app.userAgentFallback = app.userAgentFallback.replace(/ Electron\/\S+/, '').replace(/ Flecha\/\S+/i, '');

class Browser implements TabHost {
  readonly win: BaseWindow;
  private ui: WebContentsView;
  private views = new Map<string, WebContentsView>();
  /** Tab views on screen, left to right (two when a split is showing). */
  private shown: WebContentsView[] = [];
  private peek?: WebContentsView;
  private paletteOpen = false;
  private findView?: WebContentsView;
  /** The page being searched; find closes when a different tab comes on screen. */
  private findTarget?: WebContents;
  private lastSelected?: WebContents;

  constructor(
    private store: Store,
    private history: History,
    private extensions: ElectronChromeExtensions,
  ) {
    const saved = store.state.window;
    // Only reuse the old position if it is still on a connected display.
    const onScreen =
      saved &&
      screen.getAllDisplays().some(({ workArea: a }) => saved.x < a.x + a.width && saved.x + saved.width > a.x && saved.y < a.y + a.height && saved.y + saved.height > a.y);
    this.win = new BaseWindow({
      ...(onScreen ? { x: saved.x, y: saved.y, width: saved.width, height: saved.height } : { width: 1440, height: 900 }),
      minWidth: 720,
      minHeight: 420,
      titleBarStyle: 'hidden',
      trafficLightPosition: { x: 16, y: 18 },
      backgroundColor: store.activeSpace.color,
    });

    this.ui = new WebContentsView({
      webPreferences: {
        preload: path.join(__dirname, '../preload/preload.js'),
        sandbox: true,
        contextIsolation: true,
      },
    });
    this.ui.setBackgroundColor('#00000000');
    this.win.contentView.addChildView(this.ui);
    this.ui.webContents.loadFile(path.join(__dirname, '../renderer/index.html'));

    if (onScreen && saved.maximized) this.win.maximize();
    this.win.on('resize', () => this.layout());
    const saveBounds = () => this.saveBounds();
    this.win.on('resize', saveBounds);
    this.win.on('move', saveBounds);
    this.win.on('maximize', saveBounds);
    this.win.on('unmaximize', saveBounds);
    // Tab views are gone once the window has closed, so take the snapshot just before.
    this.win.on('close', () => this.snapshotTabs());
    this.win.on('closed', () => app.quit());
    store.onChange(() => this.sync());
    this.sync();
  }

  get uiContents(): WebContents {
    return this.ui.webContents;
  }

  // ---- tabs ---------------------------------------------------------------

  openTab(input: string, opts: { activate?: boolean } = {}) {
    const url = resolveInput(input);
    return this.store.addTab(url, { activate: opts.activate, afterTabId: opts.activate === false ? this.store.activeSpace.activeTabId : undefined });
  }

  // ---- TabHost, for chrome.tabs calls from extensions ----------------------

  createTabFor(url?: string) {
    const tab = this.openTab(url ?? 'about:blank');
    return this.viewFor(tab.id).webContents;
  }

  selectTabFor(wc: WebContents) {
    const id = this.tabIdFor(wc);
    if (id) this.store.activate(id);
  }

  removeTabFor(wc: WebContents) {
    const id = this.tabIdFor(wc);
    if (id) this.closeTab(id);
  }

  private tabIdFor(wc: WebContents) {
    for (const [id, view] of this.views) if (view.webContents === wc) return id;
    return undefined;
  }

  splitNew(input: string) {
    const tab = this.openTab(input, { activate: false });
    this.store.splitWith(tab.id);
  }

  navigate(input: string) {
    const tab = this.store.activeTab;
    if (!tab) return this.openTab(input);
    this.viewFor(tab.id).webContents.loadURL(resolveInput(input));
  }

  closeTab(tabId: string) {
    // Pinned tabs and favorites stay in the sidebar; either way the page is unloaded.
    this.store.closeTab(tabId);
    this.destroyView(tabId);
  }

  activeContents(): WebContents | undefined {
    const tab = this.store.activeTab;
    return tab ? this.views.get(tab.id)?.webContents : undefined;
  }

  cycleTab(step: 1 | -1) {
    const space = this.store.activeSpace;
    const ids = this.store.allTabIds(space);
    if (!ids.length) return;
    const i = space.activeTabId ? ids.indexOf(space.activeTabId) : -1;
    this.store.activate(ids[(i + step + ids.length) % ids.length]);
  }

  destroyView(tabId: string) {
    const view = this.views.get(tabId);
    if (!view) return;
    if (this.shown.includes(view)) {
      this.win.contentView.removeChildView(view);
      this.shown = this.shown.filter((v) => v !== view);
    }
    this.views.delete(tabId);
    view.webContents.close();
    this.sync();
  }

  /** Views are created lazily, so imported or pinned tabs cost nothing until opened. */
  private viewFor(tabId: string): WebContentsView {
    const existing = this.views.get(tabId);
    if (existing) return existing;

    const view = new WebContentsView({ webPreferences: { sandbox: true, contextIsolation: true } });
    view.setBorderRadius(10);
    const wc = view.webContents;
    const update = (patch: Parameters<Store['updateTab']>[1]) => this.store.updateTab(tabId, patch);

    wc.on('page-title-updated', (_e, title) => {
      update({ title });
      this.history.updateTitle(wc.getURL(), title);
    });
    wc.on('page-favicon-updated', (_e, favicons) => update({ favicon: favicons[0] }));
    wc.on('did-navigate', (_e, url) => {
      update({ url, history: snapshotHistory(wc) });
      this.history.record(url, wc.getTitle());
    });
    wc.on('did-navigate-in-page', (_e, url, isMainFrame) => isMainFrame && update({ url, history: snapshotHistory(wc) }));
    wc.on('did-start-loading', () => update({ loading: true }));
    wc.on('did-stop-loading', () => {
      update({ loading: false });
      this.sendState();
    });
    wc.on('context-menu', (_e, params) => this.pageMenu(wc, params));
    wc.on('found-in-page', (_e, result) => {
      if (wc === this.findTarget) {
        this.findView?.webContents.send('find:result', { active: result.activeMatchOrdinal, total: result.matches });
      }
    });
    // Clicking into the other half of a split makes it the active tab.
    wc.on('focus', () => {
      const space = this.store.activeSpace;
      if (space.split?.includes(tabId) && space.activeTabId !== tabId) this.store.activate(tabId);
    });
    wc.setWindowOpenHandler(({ url, disposition }) => {
      // Real popups (OAuth, payment) need window.opener, so let them open as windows.
      if (disposition === 'new-window') return { action: 'allow', overrideBrowserWindowOptions: { width: 520, height: 720 } };
      // Like Arc, links leaving a pinned tab or favorite open in Peek instead of a new tab.
      const zone = this.store.locate(tabId)?.zone;
      if (disposition === 'foreground-tab' && zone && zone !== 'today') {
        this.openPeek(url);
        return { action: 'deny' };
      }
      this.openTab(url, { activate: disposition !== 'background-tab' });
      return { action: 'deny' };
    });

    this.views.set(tabId, view);
    this.extensions.addTab(wc, this.win);
    const tab = this.store.state.tabs[tabId];
    const url = tab?.url ?? 'about:blank';
    if (tab?.history?.entries.length) wc.navigationHistory.restore(tab.history).catch(() => wc.loadURL(url));
    else wc.loadURL(url);
    return view;
  }

  // ---- peek -----------------------------------------------------------------

  // ---- find in page ----------------------------------------------------------

  openFind() {
    const target = this.activeContents();
    if (!target) return;
    if (this.findTarget && this.findTarget !== target) this.findTarget.stopFindInPage('clearSelection');
    this.findTarget = target;

    if (!this.findView) {
      this.findView = new WebContentsView({
        webPreferences: { preload: path.join(__dirname, '../preload/find-preload.js'), sandbox: true, contextIsolation: true },
      });
      this.findView.setBackgroundColor('#00000000');
      this.findView.setBorderRadius(10);
      this.findView.webContents.loadFile(path.join(__dirname, '../renderer/find.html'));
    }
    const view = this.findView;
    if (!this.win.contentView.children.includes(view)) this.win.contentView.addChildView(view);
    this.layout();
    const show = () => {
      view.webContents.focus();
      view.webContents.send('find:open');
    };
    if (view.webContents.isLoading()) view.webContents.once('did-finish-load', show);
    else show();
  }

  closeFind() {
    if (!this.findView || !this.findTarget) return;
    this.findTarget.stopFindInPage('keepSelection');
    this.win.contentView.removeChildView(this.findView);
    const target = this.findTarget;
    this.findTarget = undefined;
    if (!target.isDestroyed()) target.focus();
  }

  find(text: string, forward: boolean, newSearch: boolean) {
    const target = this.findTarget;
    if (!target || target.isDestroyed()) return;
    if (!text) {
      target.stopFindInPage('clearSelection');
      this.findView?.webContents.send('find:result', { active: 0, total: 0 });
      return;
    }
    target.findInPage(text, { forward, findNext: newSearch });
  }

  get findOpen() {
    return !!this.findTarget;
  }

  get findViewContents() {
    return this.findView?.webContents;
  }

  get peekOpen() {
    return !!this.peek;
  }

  /** A floating preview above the current page, dismissed with Escape or a click outside. */
  openPeek(url: string) {
    this.closePeek();
    const view = new WebContentsView({ webPreferences: { sandbox: true, contextIsolation: true } });
    view.setBorderRadius(12);
    const wc = view.webContents;
    const send = () => this.sendPeek();
    wc.on('page-title-updated', send);
    wc.on('did-navigate', send);
    wc.on('did-navigate-in-page', send);
    wc.on('context-menu', (_e, params) => this.pageMenu(wc, params));
    wc.on('before-input-event', (event, input) => {
      if (input.type === 'keyDown' && input.key === 'Escape') {
        event.preventDefault();
        this.closePeek();
      }
    });
    wc.setWindowOpenHandler(({ url }) => {
      this.openTab(url);
      return { action: 'deny' };
    });

    this.peek = view;
    this.extensions.addTab(wc, this.win);
    // Added last so it sits above the sidebar UI, which draws the dimmed backdrop and toolbar.
    this.win.contentView.addChildView(view);
    wc.loadURL(url);
    this.layout();
    wc.focus();
  }

  closePeek() {
    const view = this.peek;
    if (!view) return;
    this.peek = undefined;
    this.win.contentView.removeChildView(view);
    view.webContents.close();
    this.layout();
    this.sendPeek();
    this.activeContents()?.focus();
  }

  /** Turns the peek into a regular tab in today. */
  promotePeek() {
    const url = this.peek?.webContents.getURL();
    if (!url) return;
    this.closePeek();
    this.openTab(url);
  }

  private sendPeek() {
    const wc = this.peek?.webContents;
    const peek: PeekState | null = wc
      ? { url: wc.getURL(), title: wc.getTitle(), bounds: this.peekBounds() }
      : null;
    this.ui.webContents.send('peek', peek);
  }

  private peekBounds() {
    const area = this.contentRect();
    const margin = Math.round(area.width * 0.08);
    return { x: area.x + margin, y: area.y + 48, width: area.width - margin * 2, height: area.height - 72 };
  }

  // ---- layout & state -----------------------------------------------------

  setPaletteOpen(open: boolean, mode: PaletteMode = 'new-tab') {
    this.paletteOpen = open;
    this.layout();
    if (open) {
      this.ui.webContents.focus();
      this.ui.webContents.send('palette', mode);
    } else {
      this.activeContents()?.focus();
    }
  }

  private sync() {
    const views = this.store.visibleTabIds().map((id) => this.viewFor(id));
    const changed = views.length !== this.shown.length || views.some((v, i) => v !== this.shown[i]);
    if (changed) {
      for (const v of this.shown) if (!views.includes(v)) this.win.contentView.removeChildView(v);
      for (const v of views) if (!this.shown.includes(v)) this.win.contentView.addChildView(v, 0);
      this.shown = views;
      if (this.findTarget && this.findTarget !== this.activeContents()) this.closeFind();
      if (!this.paletteOpen && !this.peek) this.activeContents()?.focus();
    }
    const active = this.activeContents();
    if (active && active !== this.lastSelected) {
      // Keeps chrome.tabs and the extension toolbar pointed at the tab on screen.
      this.extensions.selectTab(active);
      this.lastSelected = active;
    }
    this.win.setBackgroundColor(this.store.activeSpace.color);
    this.layout();
    this.sendState();
  }

  private contentRect() {
    const { width, height } = this.win.getContentBounds();
    const left = this.store.state.sidebarVisible ? SIDEBAR_WIDTH : GAP;
    return { x: left, y: GAP, width: width - left - GAP, height: height - GAP * 2 };
  }

  private layout() {
    const { width, height } = this.win.getContentBounds();
    // The sidebar UI covers the whole window while it draws an overlay (command bar, peek backdrop).
    const overlay = this.paletteOpen || !!this.peek;
    this.ui.setBounds({ x: 0, y: 0, width: overlay ? width : this.store.state.sidebarVisible ? SIDEBAR_WIDTH : 0, height });

    const area = this.contentRect();
    const [left, right] = this.shown;
    if (right) {
      const half = Math.floor((area.width - GAP) / 2);
      left.setBounds({ ...area, width: half });
      right.setBounds({ ...area, x: area.x + half + GAP, width: area.width - half - GAP });
    } else {
      left?.setBounds(area);
    }

    if (this.findTarget && this.findView) {
      // Top-right corner of the page being searched.
      const page = this.views.get(this.store.activeSpace.activeTabId ?? '')?.getBounds() ?? area;
      this.findView.setBounds({
        x: page.x + page.width - FIND_BAR.width - 12,
        y: page.y + 12,
        ...FIND_BAR,
      });
    }

    if (this.peek) {
      this.peek.setBounds(this.peekBounds());
      this.sendPeek();
    }
  }

  /** Records every open tab's back/forward list and scroll position, for the next launch. */
  snapshotTabs() {
    for (const [id, view] of this.views) {
      if (!view.webContents.isDestroyed()) this.store.updateTab(id, { history: snapshotHistory(view.webContents) });
    }
  }

  private saveBounds() {
    if (this.win.isFullScreen() || this.win.isMinimized()) return;
    this.store.setWindowBounds({ ...this.win.getNormalBounds(), maximized: this.win.isMaximized() });
  }

  sendState() {
    const wc = this.activeContents();
    // Navigation history is only for restoring; the sidebar doesn't need it.
    const tabs = Object.fromEntries(Object.entries(this.store.state.tabs).map(([id, { history, ...tab }]) => [id, tab]));
    const state: UiState = {
      ...this.store.state,
      tabs,
      nav: {
        canGoBack: wc?.navigationHistory.canGoBack() ?? false,
        canGoForward: wc?.navigationHistory.canGoForward() ?? false,
      },
    };
    this.ui.webContents.send('state', state);
  }

  // ---- menus --------------------------------------------------------------

  tabMenu(tabId: string) {
    const tab = this.store.state.tabs[tabId];
    const zone = this.store.locate(tabId)?.zone;
    if (!tab || !zone) return;
    const space = this.store.activeSpace;
    const splitItems: MenuItemConstructorOptions[] = [];
    if (space.split?.includes(tabId)) splitItems.push({ label: 'Exit Split View', click: () => this.store.unsplit() });
    else if (space.activeTabId && space.activeTabId !== tabId) {
      splitItems.push({ label: 'Open in Split View', click: () => this.store.splitWith(tabId) });
    }
    Menu.buildFromTemplate([
      ...splitItems,
      ...(splitItems.length ? [{ type: 'separator' as const }] : []),
      { label: zone === 'today' ? 'Pin Tab' : 'Unpin Tab', click: () => this.store.togglePin(tabId) },
      { label: zone === 'favorites' ? 'Remove from Favorites' : 'Add to Favorites', click: () => this.store.toggleFavorite(tabId) },
      { type: 'separator' },
      { label: 'Copy URL', click: () => clipboard.writeText(tab.url) },
      { label: 'Duplicate', click: () => this.openTab(tab.url) },
      ...(tab.homeUrl && tab.homeUrl !== tab.url
        ? [{ label: 'Go Back to Pinned URL', click: () => this.viewFor(tabId).webContents.loadURL(tab.homeUrl!) }]
        : []),
      { type: 'separator' },
      { label: zone === 'today' ? 'Close Tab' : 'Unload Tab', click: () => this.closeTab(tabId) },
    ]).popup({ window: this.win });
  }

  spaceMenu(spaceId: string) {
    Menu.buildFromTemplate([
      { label: 'Rename Space', click: () => this.ui.webContents.send('rename-space', spaceId) },
      { label: 'New Folder', click: () => this.store.newFolder() },
      { type: 'separator' },
      {
        label: 'Delete Space…',
        enabled: this.store.state.spaces.length > 1,
        click: async () => {
          const { response } = await dialog.showMessageBox(this.win, {
            message: 'Delete this space and all of its tabs?',
            buttons: ['Delete', 'Cancel'],
            defaultId: 1,
            cancelId: 1,
          });
          if (response === 0) for (const id of this.store.deleteSpace(spaceId)) this.destroyView(id);
        },
      },
    ]).popup({ window: this.win });
  }

  private pageMenu(wc: WebContents, params: Electron.ContextMenuParams) {
    const items: Array<MenuItemConstructorOptions | Electron.MenuItem> = [];
    if (params.linkURL) {
      items.push(
        { label: 'Open Link in New Tab', click: () => this.openTab(params.linkURL, { activate: false }) },
        { label: 'Open Link in Peek', click: () => this.openPeek(params.linkURL) },
        { label: 'Open Link in Split View', click: () => this.splitNew(params.linkURL) },
        { label: 'Copy Link', click: () => clipboard.writeText(params.linkURL) },
        { type: 'separator' },
      );
    }
    if (params.isEditable) items.push({ role: 'cut' }, { role: 'copy' }, { role: 'paste' }, { type: 'separator' });
    else if (params.selectionText) items.push({ role: 'copy' }, { type: 'separator' });
    const extensionItems = this.extensions.getContextMenuItems(wc, params);
    if (extensionItems.length) {
      for (const item of extensionItems) items.push(item);
      items.push({ type: 'separator' });
    }
    items.push(
      { label: 'Back', enabled: wc.navigationHistory.canGoBack(), click: () => wc.navigationHistory.goBack() },
      { label: 'Forward', enabled: wc.navigationHistory.canGoForward(), click: () => wc.navigationHistory.goForward() },
      { label: 'Reload', click: () => wc.reload() },
      { type: 'separator' },
      { label: 'Inspect Element', click: () => wc.inspectElement(params.x, params.y) },
    );
    Menu.buildFromTemplate(items).popup({ window: this.win });
  }

  async importArc() {
    let file = arcSidebarPath();
    try {
      readArcSidebar(file);
    } catch {
      const picked = await dialog.showOpenDialog(this.win, {
        title: 'Find Arc’s StorableSidebar.json',
        filters: [{ name: 'Arc sidebar', extensions: ['json'] }],
        properties: ['openFile'],
      });
      if (picked.canceled || !picked.filePaths[0]) return;
      file = picked.filePaths[0];
    }
    try {
      const data = readArcSidebar(file);
      this.store.importArc(data);
      const tabs = data.favorites.length + data.spaces.reduce((n, s) => n + countTabs(s.pinned) + s.today.length, 0);
      dialog.showMessageBox(this.win, { message: `Imported ${data.spaces.length} spaces and ${tabs} tabs from Arc.` });
    } catch (err) {
      dialog.showErrorBox('Arc import failed', String(err instanceof Error ? err.message : err));
    }
  }
}

function snapshotHistory(wc: WebContents): TabInfo['history'] {
  const entries = wc.navigationHistory.getAllEntries();
  const index = wc.navigationHistory.getActiveIndex();
  const start = Math.max(0, Math.min(index, entries.length - SAVED_HISTORY));
  return {
    entries: entries.slice(start, start + SAVED_HISTORY).map(({ url, title, pageState }) => ({ url, title, pageState })),
    index: index - start,
  };
}

function countTabs(nodes: Array<{ kind: string; children?: any[] }>): number {
  return nodes.reduce((n, node) => n + (node.kind === 'tab' ? 1 : countTabs(node.children ?? [])), 0);
}

// ---- app wiring ------------------------------------------------------------

function buildMenu(browser: Browser, store: Store, settings: Settings) {
  const active = () => browser.activeContents();
  const template: MenuItemConstructorOptions[] = [
    {
      label: app.name,
      submenu: [
        { role: 'about' },
        { type: 'separator' },
        { label: 'Settings…', accelerator: 'CmdOrCtrl+,', click: () => settings.open() },
        { type: 'separator' },
        { role: 'services' },
        { type: 'separator' },
        { role: 'hide' },
        { role: 'hideOthers' },
        { role: 'unhide' },
        { type: 'separator' },
        { role: 'quit' },
      ],
    },
    {
      label: 'File',
      submenu: [
        { label: 'New Tab', accelerator: 'CmdOrCtrl+T', click: () => browser.setPaletteOpen(true, 'new-tab') },
        { label: 'Open Location', accelerator: 'CmdOrCtrl+L', click: () => browser.setPaletteOpen(true, 'navigate') },
        { label: 'New Space', accelerator: 'Ctrl+Shift+N', click: () => store.newSpace() },
        {
          label: 'Close Tab',
          accelerator: 'CmdOrCtrl+W',
          click: () => {
            if (browser.peekOpen) browser.closePeek();
            else if (store.activeTab) browser.closeTab(store.activeTab.id);
          },
        },
        {
          label: 'Reopen Closed Tab',
          accelerator: 'CmdOrCtrl+Shift+T',
          click: () => {
            const item = store.popArchive();
            if (item) browser.openTab(item.url);
          },
        },
        { type: 'separator' },
        { label: 'Import from Arc…', click: () => browser.importArc() },
      ],
    },
    {
      label: 'Edit',
      submenu: [
        { role: 'undo' },
        { role: 'redo' },
        { type: 'separator' },
        { role: 'cut' },
        { role: 'copy' },
        { role: 'paste' },
        { role: 'pasteAndMatchStyle' },
        { role: 'selectAll' },
        { type: 'separator' },
        { label: 'Find…', accelerator: 'CmdOrCtrl+F', click: () => browser.openFind() },
        { label: 'Find Next', accelerator: 'CmdOrCtrl+G', click: () => findAgain(browser, true) },
        { label: 'Find Previous', accelerator: 'CmdOrCtrl+Shift+G', click: () => findAgain(browser, false) },
      ],
    },
    {
      label: 'View',
      submenu: [
        { label: 'Toggle Sidebar', accelerator: 'CmdOrCtrl+S', click: () => store.setSidebarVisible(!store.state.sidebarVisible) },
        { type: 'separator' },
        { label: 'Split View with New Tab', accelerator: 'Ctrl+Shift+=', click: () => browser.setPaletteOpen(true, 'split') },
        { label: 'Exit Split View', accelerator: 'Ctrl+Shift+-', click: () => store.unsplit() },
        { type: 'separator' },
        { label: 'Reload', accelerator: 'CmdOrCtrl+R', click: () => active()?.reload() },
        { label: 'Hard Reload', accelerator: 'CmdOrCtrl+Shift+R', click: () => active()?.reloadIgnoringCache() },
        { label: 'Back', accelerator: 'CmdOrCtrl+[', click: () => active()?.navigationHistory.goBack() },
        { label: 'Forward', accelerator: 'CmdOrCtrl+]', click: () => active()?.navigationHistory.goForward() },
        { type: 'separator' },
        { label: 'Zoom In', accelerator: 'CmdOrCtrl+=', click: () => zoom(active(), 0.5) },
        { label: 'Zoom Out', accelerator: 'CmdOrCtrl+-', click: () => zoom(active(), -0.5) },
        { label: 'Actual Size', accelerator: 'CmdOrCtrl+0', click: () => active()?.setZoomLevel(0) },
        { type: 'separator' },
        { label: 'Developer Tools', accelerator: 'Alt+CmdOrCtrl+I', click: () => active()?.toggleDevTools() },
        { label: 'Sidebar Developer Tools', click: () => browser.uiContents.openDevTools({ mode: 'detach' }) },
        { role: 'togglefullscreen' },
      ],
    },
    {
      label: 'Tabs',
      submenu: [
        { label: 'Pin / Unpin Tab', accelerator: 'CmdOrCtrl+D', click: () => store.activeTab && store.togglePin(store.activeTab.id) },
        { label: 'Copy URL', accelerator: 'CmdOrCtrl+Shift+C', click: () => store.activeTab && clipboard.writeText(store.activeTab.url) },
        { label: 'Next Tab', accelerator: 'Ctrl+Tab', click: () => browser.cycleTab(1) },
        { label: 'Previous Tab', accelerator: 'Ctrl+Shift+Tab', click: () => browser.cycleTab(-1) },
        { type: 'separator' },
        { label: 'Clear Today Tabs', accelerator: 'CmdOrCtrl+Shift+K', click: () => clearToday(browser, store) },
      ],
    },
    {
      label: 'Spaces',
      submenu: Array.from({ length: 9 }, (_, i) => ({
        label: `Space ${i + 1}`,
        accelerator: `Ctrl+${i + 1}`,
        click: () => {
          const space = store.state.spaces[i];
          if (space) store.switchSpace(space.id);
        },
      })),
    },
    {
      label: 'Extensions',
      submenu: [
        { label: 'Chrome Web Store', click: () => browser.openTab(WEB_STORE_URL) },
        { type: 'separator' },
        ...installedExtensions().map(
          (ext): MenuItemConstructorOptions => ({
            label: ext.name,
            submenu: [
              { label: `Version ${ext.version}`, enabled: false },
              ...(ext.manifest.options_page || ext.manifest.options_ui
                ? [{ label: 'Options', click: () => browser.openTab(ext.url + (ext.manifest.options_ui?.page ?? ext.manifest.options_page)) }]
                : []),
              { label: 'Remove…', click: () => removeExtension(browser.win, ext.id, ext.name) },
            ],
          }),
        ),
        ...(installedExtensions().length ? [] : [{ label: 'No extensions installed', enabled: false }]),
      ],
    },
    { role: 'windowMenu' },
  ];
  Menu.setApplicationMenu(Menu.buildFromTemplate(template));
}

function zoom(wc: WebContents | undefined, delta: number) {
  wc?.setZoomLevel(wc.getZoomLevel() + delta);
}

function clearToday(browser: Browser, store: Store) {
  for (const id of store.clearToday()) browser.destroyView(id);
}

/** The last text searched, so ⌘G works after the find bar has been closed. */
let lastFind = '';

function findAgain(browser: Browser, forward: boolean) {
  if (browser.findOpen) browser.find(lastFind, forward, false);
  else browser.openFind();
}

function registerIpc(browser: Browser, store: Store, history: History, downloads: Downloads) {
  ipcMain.on('find:query', (event, text: string, forward: boolean, newSearch: boolean) => {
    if (event.sender !== browser.findViewContents) return;
    lastFind = String(text);
    browser.find(lastFind, !!forward, !!newSearch);
  });
  ipcMain.on('find:close', (event) => {
    if (event.sender === browser.findViewContents) browser.closeFind();
  });
  ipcMain.handle('search-history', (event, query: string) =>
    event.sender === browser.uiContents ? history.search(String(query)) : [],
  );
  const handlers: Record<string, (...args: any[]) => unknown> = {
    ready: () => {
      browser.sendState();
      browser.uiContents.send('downloads', downloads.items);
    },
    newTab: (input: string) => browser.openTab(input),
    navigate: (input: string) => browser.navigate(input),
    activate: (id: string) => store.activate(id),
    close: (id: string) => browser.closeTab(id),
    back: () => browser.activeContents()?.navigationHistory.goBack(),
    forward: () => browser.activeContents()?.navigationHistory.goForward(),
    reload: () => browser.activeContents()?.reload(),
    switchSpace: (id: string) => store.switchSpace(id),
    newSpace: () => store.newSpace(),
    renameSpace: (id: string, name: string) => store.renameSpace(id, name),
    tabMenu: (id: string) => browser.tabMenu(id),
    spaceMenu: (id: string) => browser.spaceMenu(id),
    toggleFolder: (id: string) => store.toggleFolder(id),
    renameFolder: (id: string, name: string) => store.renameFolder(id, name),
    moveTab: (id: string, target: DropTarget) => store.moveTab(id, target),
    clearToday: () => clearToday(browser, store),
    importArc: () => browser.importArc(),
    openPalette: (mode: PaletteMode) => browser.setPaletteOpen(true, mode),
    closePalette: () => browser.setPaletteOpen(false),
    toggleSidebar: () => store.setSidebarVisible(!store.state.sidebarVisible),
    splitWith: (id: string) => store.splitWith(id),
    splitNew: (input: string) => browser.splitNew(input),
    closePeek: () => browser.closePeek(),
    openDownload: (id: string) => downloads.open(id),
    revealDownload: (id: string) => downloads.reveal(id),
    cancelDownload: (id: string) => downloads.cancel(id),
    clearDownloads: () => downloads.clear(),
    promotePeek: () => browser.promotePeek(),
  };
  ipcMain.on('cmd', (event, name: string, ...args: unknown[]) => {
    if (event.sender !== browser.uiContents) return;
    handlers[name]?.(...args);
  });
}

function registerPermissions(browser: Browser) {
  const silent = new Set(['fullscreen', 'clipboard-sanitized-write', 'pointerLock']);
  const asked = new Set(['media', 'geolocation', 'notifications', 'display-capture', 'midi', 'clipboard-read']);
  session.defaultSession.setPermissionRequestHandler(async (wc, permission, callback, details) => {
    if (wc === browser.uiContents || !asked.has(permission)) return callback(silent.has(permission));
    const host = safeHost(details.requestingUrl);
    const { response } = await dialog.showMessageBox(browser.win, {
      message: `${host} wants to use ${permission.replace('-', ' ')}`,
      buttons: ['Allow', 'Block'],
      defaultId: 1,
      cancelId: 1,
    });
    callback(response === 0);
  });
}

function safeHost(url: string) {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

app.whenReady().then(async () => {
  const store = new Store(app.getPath('userData'));
  const history = new History(app.getPath('userData'));
  store.archiveStale(ARCHIVE_AFTER_MS);
  let browser: Browser | undefined;
  // Extensions and the ad blocker load first so they reach the very first tabs.
  const [extensions] = await Promise.all([
    setupExtensions(() => browser),
    setAdBlocking(store.state.adBlock !== false).catch((err) => console.error('Ad blocker failed', err)),
  ]);
  browser = new Browser(store, history, extensions);
  const settings = new Settings(store, () => browser!.sendState());
  buildMenu(browser, store, settings);
  const rebuildMenu = () => buildMenu(browser!, store, settings);
  session.defaultSession.extensions.on('extension-loaded', rebuildMenu);
  session.defaultSession.extensions.on('extension-unloaded', rebuildMenu);
  const downloads = new Downloads(session.defaultSession, (items, started) =>
    browser!.uiContents.send('downloads', items, started),
  );
  registerIpc(browser, store, history, downloads);
  registerPermissions(browser);

  setInterval(() => {
    for (const id of store.archiveStale(ARCHIVE_AFTER_MS)) browser.destroyView(id);
  }, 60 * 60 * 1000);
  app.on('before-quit', () => {
    browser!.snapshotTabs();
    store.saveNow();
    history.saveNow();
  });
});
