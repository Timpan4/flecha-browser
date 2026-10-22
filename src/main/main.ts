import path from 'node:path';
import {
  app,
  BaseWindow,
  nativeImage,
  nativeTheme,
  screen,
  WebContentsView,
  Menu,
  MenuItemConstructorOptions,
  clipboard,
  ClipboardItem,
  dialog,
  ipcMain,
  shell,
  session,
  type WebContents,
} from 'electron';
import type { DetectedBrowser, DropTarget, LocalServer, PaletteMode, PeekState, SearchEngineId, TabInfo, UiState } from '../shared/types';
import { Store } from './store';
import { SEARCH_ENGINES, resolveInput, searchEngine } from './url';
import { describe as describeEnvironment } from './environments';
import { addAdBlockSession, setAdBlocking } from './adblock';
import { onEachSession, sessionForSpace } from './sessions';
import { findLocalServers } from './localServers';
import { Boosts, type BoostKind } from './boosts';
import { Favicons, registerFaviconScheme } from './favicons';
import { registerPermissions } from './permissions';
import { keysFor, shortcutHints, type ShortcutId } from './shortcuts';
import { AgentBridge, type AgentTab, type ConsoleEntry } from './agentBridge';
import { History } from './history';
import { detectBrowsers, importBrowser } from './browserImport';
import { Downloads } from './downloads';
import { Settings } from './settings';
import { Flechinhas } from './flechinha';
import { canSetDefaultBrowser, deliverLinksTo, isDefaultBrowser, listenForExternalLinks, makeDefaultBrowser } from './defaultBrowser';
import { WEB_STORE_URL, installedExtensions, removeExtension, setupExtensions, type TabHost } from './extensions';
import type { ElectronChromeExtensions } from 'electron-chrome-extensions';

const SIDEBAR_WIDTH = 260;
/** With the sidebar hidden, this strip at the window's left edge brings it back on hover. */
const EDGE_WIDTH = 8;
/** The floating sidebar's inset from the window edge, and the room on its right for its shadow. */
const FLOAT_MARGIN = 10;
const FLOAT_SHADOW = 28;
/** Where the traffic lights sit so they line up with the sidebar toolbar, docked or floating. */
const TRAFFIC_LIGHTS = { x: 16, y: 18 };
const FIND_BAR = { width: 360, height: 40 };
const GAP = 8;
/** Back/forward entries kept per tab across restarts. */
const SAVED_HISTORY = 25;
/** The responsive split's phone: an iPhone-15-wide viewport with a mobile Chrome user agent. */
const PHONE = { width: 393 };
const PHONE_USER_AGENT = `Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/${process.versions.chrome} Mobile Safari/537.36`;

app.setName('Flecha');
// Lets development runs use a throwaway profile: FLECHA_USER_DATA=/tmp/flecha npm start
if (process.env.FLECHA_USER_DATA) app.setPath('userData', process.env.FLECHA_USER_DATA);
// Some sites (Google sign-in in particular) refuse user agents that mention Electron.
// One Flecha per profile: launching it again (e.g. by clicking a link) reuses this one.
const isPrimaryInstance = app.requestSingleInstanceLock();
if (!isPrimaryInstance) app.quit();
let focusBrowser = () => {};
listenForExternalLinks(() => focusBrowser());
registerFaviconScheme();

app.userAgentFallback = app.userAgentFallback.replace(/ Electron\/\S+/, '').replace(/ Flecha\/\S+/i, '');

class Browser implements TabHost {
  readonly win: BaseWindow;
  private ui: WebContentsView;
  private views = new Map<string, WebContentsView>();
  /** Tab views on screen, left to right (two when a split is showing). */
  private shown: WebContentsView[] = [];
  private peek?: WebContentsView;
  private paletteOpen = false;
  /** A tab is being dragged from the sidebar; the sidebar UI covers the page to offer split drop zones. */
  private tabDragging = false;
  /** The hidden sidebar is showing temporarily, floating over the page. */
  private sidebarPeek = false;
  private findView?: WebContentsView;
  private divider?: WebContentsView;
  private draggingDivider = false;
  /** The page being searched; find closes when a different tab comes on screen. */
  private findTarget?: WebContents;
  private lastSelected?: WebContents;

  constructor(
    private store: Store,
    private history: History,
    private boosts: Boosts,
    private extensions: ElectronChromeExtensions,
  ) {
    this.boostedHosts = boosts.hosts();
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
      trafficLightPosition: TRAFFIC_LIGHTS,
      backgroundColor: spaceTint(store.activeSpace.color),
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
    // A drag that never got its mouse-up (e.g. the window lost focus) must not leave the divider covering the pages.
    this.win.on('blur', () => this.draggingDivider && this.dragDivider('end'));
    nativeTheme.on('updated', () => {
      this.win.setBackgroundColor(spaceTint(this.store.activeSpace.color));
      this.sendState();
    });
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

  private localServers: LocalServer[] = [];
  private consoleLogs = new Map<string, ConsoleEntry[]>();
  private agentActivity?: { tabId?: string; action: string; at: number };
  private agentIdleTimer?: NodeJS.Timeout;

  // ---- AgentHost: what coding agents can do through the MCP bridge ---------

  listTabs(): AgentTab[] {
    const visible = new Set(this.store.visibleTabIds());
    const tabs: AgentTab[] = [];
    const add = (id: string, space: string, zone: AgentTab['zone']) => {
      const tab = this.store.state.tabs[id];
      if (tab) tabs.push({ id, title: tab.title, url: tab.url, space, zone, active: visible.has(id) });
    };
    for (const id of this.store.state.favorites) add(id, '', 'favorites');
    for (const space of this.store.state.spaces) {
      for (const id of this.store.allTabIds(space)) {
        if (!this.store.state.favorites.includes(id)) add(id, space.name, space.today.includes(id) ? 'today' : 'pinned');
      }
    }
    return tabs;
  }

  contentsFor(tabId?: string) {
    const id = tabId ?? this.store.activeTab?.id;
    if (!id || !this.store.state.tabs[id]) return undefined;
    return { id, wc: this.viewFor(id).webContents };
  }

  openTabFor(url: string, background: boolean) {
    return this.openTab(url, { activate: !background }).id;
  }

  activate(tabId: string) {
    this.store.activate(tabId);
  }

  close(tabId: string) {
    this.closeTab(tabId);
  }

  consoleLog(tabId: string) {
    return this.consoleLogs.get(tabId) ?? [];
  }

  showActivity(tabId: string | undefined, action: string) {
    this.agentActivity = { tabId, action, at: Date.now() };
    this.sendStateSoon();
    // Drop the "busy" highlight once the agent has been quiet for a moment.
    clearTimeout(this.agentIdleTimer);
    this.agentIdleTimer = setTimeout(() => this.sendState(), 4000);
  }
  private boostedHosts: string[] = [];
  /** Set right after startup, once the settings window exists. */
  openSettings?: (pane?: string) => void;
  /** Set right after startup; fetches icons for saved tabs that were never opened. */
  favicons?: Favicons;
  /** Other browsers to import from, looked for at startup since the app menu is built synchronously. */
  otherBrowsers: DetectedBrowser[] = [];
  private importing = false;

  /** A boost file changed: restyle matching pages in place, or reload them for scripts. */
  boostChanged({ host, kind }: { host: string; kind: BoostKind }) {
    for (const view of this.views.values()) {
      const wc = view.webContents;
      if (!this.boosts.matches(wc.getURL(), host)) continue;
      if (kind === 'styles') void this.boosts.refreshStyles(wc);
      else wc.reload();
    }
    this.boostedHosts = this.boosts.hosts();
    this.sendState();
  }

  /** Opens (creating from a template if needed) the active site's boost in your editor. */
  editBoost(kind: BoostKind) {
    const url = this.store.activeTab?.url;
    let host: string | undefined;
    try {
      host = url ? new URL(url).hostname.replace(/^www\./, '') : undefined;
    } catch {
      host = undefined;
    }
    if (!host) return void shell.openPath(this.boosts.dir);
    void shell.openPath(this.boosts.fileFor(host, kind));
    this.boostedHosts = this.boosts.hosts();
    this.sendState();
  }

  openBoostsFolder() {
    void shell.openPath(this.boosts.dir);
  }
  private consoleErrors = new Map<string, number>();
  private stateTimer?: NodeJS.Timeout;

  /** For noisy sources (console errors): one sidebar update per burst. */
  private sendStateSoon() {
    this.stateTimer ??= setTimeout(() => {
      this.stateTimer = undefined;
      this.sendState();
    }, 300);
  }

  /** Opens DevTools on the Console for the tab whose error badge was clicked. */
  showConsole(tabId: string) {
    const wc = this.views.get(tabId)?.webContents;
    if (!wc) return;
    const showPanel = () => wc.devToolsWebContents?.executeJavaScript(`DevToolsAPI.showPanel('console')`).catch(() => {});
    if (wc.isDevToolsOpened()) showPanel();
    else {
      wc.once('devtools-opened', () => setTimeout(showPanel, 300));
      wc.openDevTools({ mode: 'detach' });
    }
  }

  /** Looks for dev servers every few seconds while Flecha is in front, and rarely otherwise. */
  watchLocalServers() {
    let last = 0;
    const scan = async () => {
      const due = this.win.isFocused() ? 4000 : 30000;
      if (Date.now() - last >= due) {
        last = Date.now();
        const servers = await findLocalServers();
        if (JSON.stringify(servers) !== JSON.stringify(this.localServers)) {
          this.localServers = servers;
          this.sendState();
        }
      }
      setTimeout(scan, 2000);
    };
    this.win.on('focus', () => (last = 0));
    void scan();
  }

  focus() {
    if (this.win.isMinimized()) this.win.restore();
    this.win.show();
    this.win.focus();
    app.focus({ steal: true });
  }

  get uiContents(): WebContents {
    return this.ui.webContents;
  }

  // ---- tabs ---------------------------------------------------------------

  openTab(input: string, opts: { activate?: boolean } = {}) {
    const url = resolveInput(input, this.store.state.searchEngine);
    return this.store.addTab(url, { activate: opts.activate, afterTabId: opts.activate === false ? this.store.activeSpace.activeTabId : undefined });
  }

  /**
   * A page brought in from elsewhere (a Flechinha) becomes a Today tab in the active Space.
   * It stays loaded, scroll position and all, unless the Space keeps its own cookies: then
   * the page can't move sessions and opens again from its URL.
   */
  adoptPage(view: WebContentsView, favicon?: string) {
    const wc = view.webContents;
    const url = wc.getURL() || 'about:blank';
    if (wc.session !== sessionForSpace(this.store.activeSpace)) {
      wc.close();
      this.openTab(url);
    } else {
      const tab = this.store.addTab(url, { activate: false });
      this.store.updateTab(tab.id, { title: wc.getTitle() || url, favicon, history: snapshotHistory(wc) });
      this.wireView(tab.id, view);
      this.store.activate(tab.id);
    }
    this.focus();
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

  setTabDragging(dragging: boolean) {
    if (this.tabDragging === dragging) return;
    this.tabDragging = dragging;
    this.layout();
  }

  /** A tab dropped on the page: show it beside the page, on the side it was dropped. */
  dropToSplit(tabId: string, side: 'left' | 'right') {
    this.setTabDragging(false);
    const space = this.store.activeSpace;
    if (!space.activeTabId || space.activeTabId === tabId) return this.store.activate(tabId);
    // A responsive mirror only exists for its split; it goes away when something else takes its place.
    if (space.splitMode === 'responsive') this.exitSplit();
    this.store.splitWith(tabId, side);
  }

  splitNew(input: string) {
    const tab = this.openTab(input, { activate: false });
    this.store.splitWith(tab.id);
  }

  navigate(input: string) {
    const tab = this.store.activeTab;
    if (!tab) return this.openTab(input);
    this.viewFor(tab.id).webContents.loadURL(resolveInput(input, this.store.state.searchEngine));
  }

  closeTab(tabId: string) {
    // Pinned tabs and favorites stay in the sidebar; either way the page is unloaded.
    this.store.closeTab(tabId);
    this.destroyView(tabId);
  }

  /** A tab's page if it is loaded; never loads one. */
  loadedContents(tabId: string): WebContents | undefined {
    return this.views.get(tabId)?.webContents;
  }

  activeContents(): WebContents | undefined {
    const tab = this.store.activeTab;
    return tab ? this.views.get(tab.id)?.webContents : undefined;
  }

  /** ⌘1–⌘8 pick a tab by position (favorites, then pinned, then today); ⌘9 the last one. */
  selectTabIndex(index: number) {
    const ids = this.store.allTabIds(this.store.activeSpace);
    const id = index < 0 ? ids[ids.length - 1] : ids[index];
    if (id) this.store.activate(id);
  }

  cycleSpace(step: 1 | -1) {
    const spaces = this.store.state.spaces;
    const i = spaces.findIndex((sp) => sp.id === this.store.state.activeSpaceId);
    this.store.switchSpace(spaces[(i + step + spaces.length) % spaces.length].id);
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
    this.consoleErrors.delete(tabId);
    this.consoleLogs.delete(tabId);
    view.webContents.close();
    this.sync();
  }

  /** Views are created lazily, so imported or pinned tabs cost nothing until opened. */
  private viewFor(tabId: string): WebContentsView {
    const existing = this.views.get(tabId);
    if (existing) return existing;

    // Favorites belong to no Space, so they always use the shared session.
    const ses = sessionForSpace(this.store.locate(tabId)?.space);
    const view = new WebContentsView({ webPreferences: { session: ses, sandbox: true, contextIsolation: true } });
    this.wireView(tabId, view);
    const wc = view.webContents;
    const tab = this.store.state.tabs[tabId];
    const url = tab?.url ?? 'about:blank';
    if (tab?.history?.entries.length) wc.navigationHistory.restore(tab.history).catch(() => wc.loadURL(url));
    else wc.loadURL(url);
    return view;
  }

  /** Makes a page view the given tab's: keeps the sidebar in step with it and routes its links. */
  private wireView(tabId: string, view: WebContentsView) {
    view.setBorderRadius(10);
    const wc = view.webContents;
    const update = (patch: Parameters<Store['updateTab']>[1]) => this.store.updateTab(tabId, patch);

    wc.on('audio-state-changed', (event) => update({ audible: event.audible }));
    wc.on('page-title-updated', (_e, title) => {
      update({ title });
      this.history.updateTitle(wc.getURL(), title);
    });
    wc.on('page-favicon-updated', (_e, favicons) => update({ favicon: favicons[0] }));
    wc.on('dom-ready', () => void this.boosts.apply(wc));
    wc.on('console-message', (event) => {
      const log = this.consoleLogs.get(tabId) ?? [];
      log.push({ level: event.level, message: event.message, source: event.sourceId ? `${event.sourceId}:${event.lineNumber}` : '', time: Date.now() });
      this.consoleLogs.set(tabId, log.slice(-300));
      if (event.level !== 'error') return;
      this.consoleErrors.set(tabId, (this.consoleErrors.get(tabId) ?? 0) + 1);
      this.sendStateSoon();
    });
    wc.on('did-start-navigation', (details) => {
      if (details.isMainFrame && !details.isSameDocument && this.consoleErrors.delete(tabId)) this.sendStateSoon();
    });
    wc.on('did-navigate', (_e, url) => {
      update({ url, history: snapshotHistory(wc) });
      this.history.record(url, wc.getTitle());
      this.followInResponsiveSplit(tabId, url);
    });
    wc.on('did-navigate-in-page', (_e, url, isMainFrame) => {
      if (!isMainFrame) return;
      update({ url, history: snapshotHistory(wc) });
      this.followInResponsiveSplit(tabId, url);
    });
    if (this.store.state.tabs[tabId]?.device === 'mobile') emulatePhone(wc);
    if (this.store.state.tabs[tabId]?.muted) wc.setAudioMuted(true);
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
      if (disposition === 'foreground-tab' && zone && zone !== 'today' && this.store.state.peekPinnedLinks !== false) {
        this.openPeek(url, wc.session);
        return { action: 'deny' };
      }
      this.openTab(url, { activate: disposition !== 'background-tab' });
      return { action: 'deny' };
    });

    this.views.set(tabId, view);
    if (wc.session === session.defaultSession) this.extensions.addTab(wc, this.win);
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

  /** Moves the active tab to the same page on the project's next environment. */
  nextEnvironment() {
    const tab = this.store.activeTab;
    const env = tab && describeEnvironment(tab.url, this.store.state.environments);
    if (!tab || !env?.switches.length) return;
    this.viewFor(tab.id).webContents.loadURL(env.switches[0].url);
  }

  toggleResponsive() {
    if (this.store.activeSpace.splitMode === 'responsive') return this.exitSplit();
    this.store.openResponsive();
  }

  exitSplit() {
    const mirror = this.store.unsplit();
    if (mirror) this.closeTab(mirror);
  }

  /**
   * In a responsive split the phone follows the desktop side. Only that direction: sites
   * often redirect phones to an m. domain, which must not drag the desktop side along.
   */
  private followInResponsiveSplit(tabId: string, url: string) {
    const space = this.store.locate(tabId)?.space;
    if (space?.splitMode !== 'responsive' || space.split?.[0] !== tabId) return;
    const view = this.views.get(space.split[1]);
    if (view && view.webContents.getURL() !== url) view.webContents.loadURL(url);
  }

  setSidebarPeek(peek: boolean) {
    if (this.store.state.sidebarVisible || this.sidebarPeek === peek) return;
    this.sidebarPeek = peek;
    this.layout();
    this.sendState();
  }

  /** During the tour, a shortcut is reported to it instead of acting. Returns whether it was. */
  private tourKey(command: string) {
    if (this.store.state.onboarded) return false;
    this.ui.webContents.send('tour-key', command);
    return true;
  }

  toggleSidebar() {
    if (this.tourKey('toggleSidebar')) return;
    this.sidebarPeek = false;
    this.store.setSidebarVisible(!this.store.state.sidebarVisible);
  }

  flash(message: string) {
    this.ui.webContents.send('flash', message);
  }

  async capturePage() {
    const wc = this.activeContents();
    if (!wc) return;
    const png = (await wc.capturePage()).toPNG();
    await clipboard.write([new ClipboardItem({ 'image/png': new Blob([new Uint8Array(png)], { type: 'image/png' }) })]);
    this.flash('Screenshot copied');
  }

  /** Signs out of one site: its cookies, storage and cache, in the session the tab uses. */
  async clearSiteData(origin: string) {
    const wc = this.activeContents();
    if (!wc) return;
    await wc.session.clearStorageData({ origin });
    wc.reload();
    this.flash('Site data cleared');
  }

  zoom(step: 1 | -1 | 0) {
    const wc = this.activeContents();
    if (!wc) return;
    wc.setZoomLevel(step === 0 ? 0 : wc.getZoomLevel() + step * 0.5);
    this.sendState();
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
  openPeek(url: string, ses = this.activeContents()?.session ?? session.defaultSession) {
    this.closePeek();
    const view = new WebContentsView({ webPreferences: { session: ses, sandbox: true, contextIsolation: true } });
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
    if (wc.session === session.defaultSession) this.extensions.addTab(wc, this.win);
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
    // The welcome tour asks you to try the real shortcuts and shows what they do.
    if (open && this.tourKey({ 'new-tab': 'newTab', navigate: 'openLocation', split: 'splitNew' }[mode])) return;
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
      if (active.session === session.defaultSession) this.extensions.selectTab(active);
      this.lastSelected = active;
    }
    this.win.setBackgroundColor(spaceTint(this.store.activeSpace.color));
    this.layout();
    this.sendState();
  }

  /** The draggable strip in the gap between split panes (not for the fixed-width phone). */
  private placeDivider(area: ReturnType<Browser['contentRect']> | undefined, leftWidth: number) {
    const show = !!area && this.store.activeSpace.splitMode !== 'responsive';
    if (!show) {
      if (this.divider && this.win.contentView.children.includes(this.divider)) this.win.contentView.removeChildView(this.divider);
      return;
    }
    if (!this.divider) {
      this.divider = new WebContentsView({
        webPreferences: { preload: path.join(__dirname, '../preload/divider-preload.js'), sandbox: true, contextIsolation: true },
      });
      this.divider.setBackgroundColor('#00000000');
      this.divider.webContents.loadFile(path.join(__dirname, '../renderer/divider.html'));
    }
    if (!this.win.contentView.children.includes(this.divider)) this.win.contentView.addChildView(this.divider, this.shown.length);
    // While dragging it covers both panes, so the pages can't take the mouse away.
    this.divider.setBounds(this.draggingDivider ? area! : { x: area!.x + leftWidth, y: area!.y, width: GAP, height: area!.height });
  }

  get dividerContents() {
    return this.divider?.webContents;
  }

  dragDivider(phase: 'start' | 'end' | 'reset' | number) {
    if (phase === 'start' || phase === 'end') this.draggingDivider = phase === 'start';
    else if (phase === 'reset') this.store.setSplitRatio(0.5);
    else if (this.divider) {
      // The view may still be the thin strip or already cover the panes; its x says which.
      const area = this.contentRect();
      const x = this.divider.getBounds().x - area.x + phase;
      this.store.setSplitRatio((x - GAP / 2) / (area.width - GAP));
    }
    this.layout();
  }

  private contentRect() {
    const { width, height } = this.win.getContentBounds();
    const left = this.store.state.sidebarVisible ? SIDEBAR_WIDTH : GAP;
    return { x: left, y: GAP, width: width - left - GAP, height: height - GAP * 2 };
  }

  private layout() {
    const { width, height } = this.win.getContentBounds();
    // The sidebar UI covers the whole window while it draws an overlay (command bar, peek backdrop).
    const overlay = this.paletteOpen || !!this.peek || this.tabDragging || !this.store.state.onboarded;
    // With no page on screen the sidebar UI also draws the empty state where the page would be.
    const fullWidth = overlay || !this.shown.length;
    const sidebarWidth = this.store.state.sidebarVisible
      ? SIDEBAR_WIDTH
      : this.sidebarPeek
        ? FLOAT_MARGIN + SIDEBAR_WIDTH + FLOAT_SHADOW
        : EDGE_WIDTH;
    this.ui.setBounds({ x: 0, y: 0, width: fullWidth ? width : sidebarWidth, height });
    // With the sidebar out of the way the traffic lights would sit on the page; they come back with it,
    // moved in by the floating sidebar's inset so they stay level with its toolbar.
    const floating = !this.store.state.sidebarVisible && this.sidebarPeek;
    this.win.setWindowButtonVisibility(this.store.state.sidebarVisible || this.sidebarPeek);
    const inset = floating ? FLOAT_MARGIN : 0;
    this.win.setWindowButtonPosition({ x: TRAFFIC_LIGHTS.x + inset, y: TRAFFIC_LIGHTS.y + inset });

    const area = this.contentRect();
    const [left, right] = this.shown;
    if (right && this.store.activeSpace.splitMode === 'responsive') {
      left.setBounds({ ...area, width: area.width - PHONE.width - GAP });
      right.setBounds({ ...area, x: area.x + area.width - PHONE.width, width: PHONE.width });
      this.placeDivider(area, area.width - PHONE.width - GAP);
    } else if (right) {
      const leftWidth = Math.round((area.width - GAP) * (this.store.activeSpace.splitRatio ?? 0.5));
      left.setBounds({ ...area, width: leftWidth });
      right.setBounds({ ...area, x: area.x + leftWidth + GAP, width: area.width - leftWidth - GAP });
      this.placeDivider(area, leftWidth);
    } else {
      this.placeDivider(undefined, 0);
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
      searchEngineName: searchEngine(this.store.state.searchEngine).name,
      tint: spaceTint(this.store.activeSpace.color),
      sidebarMode: this.store.state.sidebarVisible ? 'docked' : this.sidebarPeek ? 'floating' : 'hidden',
      environment: this.store.activeTab ? describeEnvironment(this.store.activeTab.url, this.store.state.environments) : undefined,
      localServers: this.localServers,
      consoleErrors: Object.fromEntries(this.consoleErrors),
      boostedHosts: this.boostedHosts,
      faviconHosts: this.favicons?.hosts() ?? [],
      shortcuts: shortcutHints(this.store.state),
      defaultBrowser: { isDefault: isDefaultBrowser(), canSet: canSetDefaultBrowser() },
      agent: this.agentActivity && {
        connected: Date.now() - this.agentActivity.at < 60_000,
        busy: Date.now() - this.agentActivity.at < 3_500,
        tabId: this.agentActivity.tabId,
        action: this.agentActivity.action,
      },
      nav: {
        canGoBack: wc?.navigationHistory.canGoBack() ?? false,
        canGoForward: wc?.navigationHistory.canGoForward() ?? false,
        zoom: wc ? Math.round(wc.getZoomFactor() * 100) : 100,
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
    if (space.split?.includes(tabId)) splitItems.push({ label: 'Exit Split View', click: () => this.exitSplit() });
    else if (space.activeTabId && space.activeTabId !== tabId) {
      splitItems.push({ label: 'Open in Split View', click: () => this.store.splitWith(tabId) });
    }
    const name = tab.customTitle ?? tab.title;
    const otherSpaces = this.store.state.spaces.filter((s) => s.id !== this.store.locate(tabId)?.space?.id);
    const copy = (text: string, message: string) => {
      clipboard.writeText(text);
      this.flash(message);
    };
    const closeMany = (which: 'below' | 'others') => {
      for (const id of this.store.closeTodayTabs(tabId, which)) this.destroyView(id);
    };
    Menu.buildFromTemplate([
      ...splitItems,
      ...(splitItems.length ? [{ type: 'separator' as const }] : []),
      { label: 'Copy Link', click: () => copy(tab.url, 'Link copied') },
      { label: 'Copy as Markdown Link', click: () => copy(`[${name}](${tab.url})`, 'Markdown link copied') },
      { label: 'Rename…', click: () => this.ui.webContents.send('rename-tab', tabId) },
      { label: 'Duplicate', click: () => this.openTab(tab.url) },
      { type: 'separator' },
      { label: zone === 'today' ? 'Pin Tab' : 'Unpin Tab', click: () => this.store.togglePin(tabId) },
      { label: zone === 'favorites' ? 'Remove from Favorites' : 'Add to Favorites', click: () => this.store.toggleFavorite(tabId) },
      {
        label: 'Move to Space',
        enabled: zone !== 'favorites' && otherSpaces.length > 0,
        submenu: otherSpaces.map((s) => ({
          label: `${s.emoji ? s.emoji + '  ' : ''}${s.name}`,
          click: () => {
            // A Space with its own cookies needs the page reopened in its session.
            this.destroyView(tabId);
            this.store.moveToSpace(tabId, s.id);
          },
        })),
      },
      { type: 'separator' },
      { label: 'Reload', enabled: this.views.has(tabId), click: () => this.views.get(tabId)?.webContents.reload() },
      { label: tab.muted ? 'Unmute Tab' : 'Mute Tab', click: () => this.toggleMute(tabId) },
      ...(tab.homeUrl && tab.homeUrl !== tab.url
        ? [{ label: 'Go Back to Pinned URL', click: () => this.viewFor(tabId).webContents.loadURL(tab.homeUrl!) }]
        : []),
      { type: 'separator' },
      { label: zone === 'today' ? 'Close Tab' : 'Unload Tab', click: () => this.closeTab(tabId) },
      ...(zone === 'today'
        ? [
            { label: 'Close Tabs Below', click: () => closeMany('below') },
            { label: 'Close Other Tabs', click: () => closeMany('others') },
          ]
        : []),
    ]).popup({ window: this.win });
  }

  toggleMute(tabId: string) {
    const tab = this.store.state.tabs[tabId];
    const wc = this.views.get(tabId)?.webContents;
    if (!tab) return;
    wc?.setAudioMuted(!tab.muted);
    this.store.updateTab(tabId, { muted: !tab.muted });
  }

  private async clearSpaceData(spaceId: string) {
    const space = this.store.state.spaces.find((s) => s.id === spaceId);
    if (!space?.isolated) return;
    const { response } = await dialog.showMessageBox(this.win, {
      message: `Sign out of everything in “${space.name}”?`,
      detail: 'Clears cookies, storage and cache for this Space only.',
      buttons: ['Clear', 'Cancel'],
      defaultId: 1,
      cancelId: 1,
    });
    if (response !== 0) return;
    await sessionForSpace(space).clearStorageData();
    for (const id of this.store.allTabIds(space)) if (!this.store.state.favorites.includes(id)) this.views.get(id)?.webContents.reload();
  }

  spaceMenu(spaceId: string) {
    const space = this.store.state.spaces.find((s) => s.id === spaceId);
    Menu.buildFromTemplate([
      { label: 'Edit Space…', click: () => this.editSpace(spaceId) },
      { label: 'New Folder', click: () => this.store.newFolder() },
      { type: 'separator' },
      {
        label: 'Separate Cookies and Logins',
        type: 'checkbox',
        checked: !!space?.isolated,
        click: () => this.setSpaceIsolated(spaceId, !space?.isolated),
      },
      ...(space?.isolated
        ? [{ label: 'Clear Cookies and Site Data…', click: () => this.clearSpaceData(spaceId) }]
        : []),
      { type: 'separator' },
      { label: 'Delete Space…', enabled: this.store.state.spaces.length > 1, click: () => this.deleteSpace(spaceId) },
    ]).popup({ window: this.win });
  }

  /** Opens the sidebar's Space editor (name, emoji, colour, cookies). */
  editSpace(spaceId: string) {
    if (this.store.state.activeSpaceId !== spaceId) this.store.switchSpace(spaceId);
    this.ui.webContents.send('edit-space', spaceId);
  }

  newSpace() {
    this.editSpace(this.store.newSpace().id);
  }

  setSpaceIsolated(spaceId: string, isolated: boolean) {
    // Pages must reopen in the other session.
    for (const id of this.store.setIsolated(spaceId, isolated)) this.destroyView(id);
  }

  async deleteSpace(spaceId: string) {
    const space = this.store.state.spaces.find((s) => s.id === spaceId);
    if (!space || this.store.state.spaces.length < 2) return;
    const { response } = await dialog.showMessageBox(this.win, {
      message: `Delete “${space.name}”?`,
      detail: 'Its pinned and today tabs are closed. This can’t be undone.',
      buttons: ['Delete Space', 'Cancel'],
      defaultId: 1,
      cancelId: 1,
    });
    if (response === 0) for (const id of this.store.deleteSpace(spaceId)) this.destroyView(id);
  }

  private pageMenu(wc: WebContents, params: Electron.ContextMenuParams) {
    const items: Array<MenuItemConstructorOptions | Electron.MenuItem> = [];
    if (params.linkURL) {
      items.push(
        { label: 'Open Link in New Tab', click: () => this.openTab(params.linkURL, { activate: false }) },
        { label: 'Open Link in Peek', click: () => this.openPeek(params.linkURL, wc.session) },
        { label: 'Open Link in Split View', click: () => this.splitNew(params.linkURL) },
        { label: 'Copy Link', click: () => clipboard.writeText(params.linkURL) },
        { type: 'separator' },
      );
    }
    if (params.isEditable) items.push({ role: 'cut' }, { role: 'copy' }, { role: 'paste' }, { type: 'separator' });
    else if (params.selectionText) items.push({ role: 'copy' }, { type: 'separator' });
    // Extensions only run in the shared session, not in Spaces with separate cookies.
    const extensionItems = wc.session === session.defaultSession ? this.extensions.getContextMenuItems(wc, params) : [];
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

  /** One item per browser found, for both the File menu and the command bar's chooser. */
  importItems(): MenuItemConstructorOptions[] {
    if (!this.otherBrowsers.length) return [{ label: 'No Other Browsers Found', enabled: false }];
    return [
      ...this.otherBrowsers.map(
        (b): MenuItemConstructorOptions => ({
          label: `${b.name}…`,
          sublabel: [b.hasBookmarks && (b.id === 'arc' ? 'Spaces' : 'Bookmarks'), b.hasHistory && 'history'].filter(Boolean).join(' and '),
          click: () => void this.importFrom(b),
        }),
      ),
      { type: 'separator' },
      { label: 'Passwords and cookies stay in the other browser', enabled: false },
    ];
  }

  /** The command bar's "Import from another browser": looks again, then offers what it found. */
  async importMenu() {
    this.otherBrowsers = await detectBrowsers();
    Menu.buildFromTemplate(this.importItems()).popup({ window: this.win });
  }

  /** Brings bookmarks and history over, then says what came across and what didn't. */
  async importFrom(source: DetectedBrowser) {
    if (this.importing) return;
    this.importing = true;
    try {
      const result = await importBrowser(source.id, this.store, this.history);
      const lines = [
        result.spaces && `${plural(result.spaces, 'new Space')} with ${plural(result.bookmarks, 'pinned tab')}`,
        result.history && `${plural(result.history, 'page')} of history`,
      ].filter(Boolean);
      dialog.showMessageBox(this.win, {
        type: result.warnings.length ? 'warning' : 'info',
        message: lines.length ? `Imported from ${source.name}` : `Nothing was imported from ${source.name}`,
        detail: [
          ...lines,
          ...(result.warnings.length ? ['', ...result.warnings] : []),
          '',
          `Passwords and cookies stay in ${source.name}. To bring passwords along, use a password manager.`,
        ].join('\n'),
      });
    } finally {
      this.importing = false;
    }
  }
}

/**
 * Phone emulation through the DevTools protocol, the same mechanism as Chrome's device
 * mode: a mobile viewport (so `width=device-width` works), touch input and a mobile user
 * agent. Electron's own enableDeviceEmulation leaves the layout viewport at 980px.
 */
function emulatePhone(wc: WebContents) {
  wc.setUserAgent(PHONE_USER_AGENT);
  try {
    wc.debugger.attach('1.3');
  } catch {
    // Already attached (DevTools open); the commands still go through.
  }
  const send = (method: string, params: object) => wc.debugger.sendCommand(method, params).catch(() => {});
  // Height 0 keeps the pane's real height.
  send('Emulation.setDeviceMetricsOverride', { width: PHONE.width, height: 0, deviceScaleFactor: 3, mobile: true });
  send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 5 });
  send('Emulation.setEmitTouchEventsForMouse', { enabled: true, configuration: 'mobile' });
}

/** Space colours are pastels; in dark mode they become a deep tint of the same hue. */
function spaceTint(color: string) {
  if (!nativeTheme.shouldUseDarkColors) return color;
  const base = [17, 24, 36];
  const rgb = [1, 3, 5].map((i) => parseInt(color.slice(i, i + 2), 16));
  const mixed = rgb.map((c, i) => Math.round(c * 0.16 + base[i] * 0.84));
  return '#' + mixed.map((c) => c.toString(16).padStart(2, '0')).join('');
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

function plural(n: number, word: string) {
  return `${n.toLocaleString()} ${word}${n === 1 ? '' : 's'}`;
}

// ---- app wiring ------------------------------------------------------------

function buildMenu(browser: Browser, store: Store, settings: Settings, flechinhas: Flechinhas) {
  // Page commands act on a Flechinha's page while one is in front.
  const active = () => flechinhas.focused()?.pageContents ?? browser.activeContents();
  const tab = () => store.activeTab;
  const space = store.activeSpace;
  const copy = (text: string | undefined, message: string) => {
    if (!text) return;
    clipboard.writeText(text);
    browser.flash(message);
  };
  const archiveAfter = store.state.archiveAfterHours ?? 12;
  const k = (id: ShortcutId) => keysFor(store.state, id);
  const template: MenuItemConstructorOptions[] = [
    {
      label: app.name,
      submenu: [
        { role: 'about', label: `About ${app.name}` },
        { type: 'separator' },
        { label: 'Settings…', accelerator: k('settings'), click: () => settings.open() },
        {
          label: 'Make Flecha Your Default Browser',
          enabled: canSetDefaultBrowser() && !isDefaultBrowser(),
          click: () => makeDefaultBrowser(),
        },
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
        {
          label: 'New Tab',
          accelerator: k('newTab'),
          click: () => {
            // From a Flechinha, the new tab is Flecha's: bring the main window forward for it.
            if (flechinhas.focused()) browser.focus();
            browser.setPaletteOpen(true, 'new-tab');
          },
        },
        {
          label: 'Open Location…',
          accelerator: k('openLocation'),
          click: () => (flechinhas.focused() ? flechinhas.focused()!.editAddress() : browser.setPaletteOpen(true, 'navigate')),
        },
        { label: 'New Space', accelerator: k('newSpace'), click: () => browser.newSpace() },
        { label: 'New Flechinha Window', accelerator: k('newFlechinha'), click: () => flechinhas.create() },
        { type: 'separator' },
        {
          label: 'Close Tab',
          accelerator: k('closeTab'),
          click: () => {
            if (flechinhas.focused()) flechinhas.focused()!.close();
            else if (browser.peekOpen) browser.closePeek();
            else if (tab()) browser.closeTab(tab()!.id);
          },
        },
        {
          label: 'Reopen Closed Tab',
          accelerator: k('reopenTab'),
          click: () => {
            const item = store.popArchive();
            if (item) browser.openTab(item.url);
          },
        },
        { type: 'separator' },
        {
          label: 'Share',
          submenu: [
            {
              label: 'Copy Link',
              accelerator: k('copyLink'),
              click: () => (flechinhas.focused() ? flechinhas.focused()!.copyLink() : copy(tab()?.url, 'Link copied')),
            },
            {
              label: 'Copy as Markdown Link',
              accelerator: k('copyMarkdown'),
              click: () => tab() && copy(`[${tab()!.customTitle ?? tab()!.title}](${tab()!.url})`, 'Markdown link copied'),
            },
            { label: 'Copy Screenshot', accelerator: k('screenshot'), click: () => void browser.capturePage() },
          ],
        },
        { label: 'Print…', accelerator: 'CmdOrCtrl+P', click: () => active()?.print() },
        { type: 'separator' },
        { label: 'Import Browser Data', submenu: browser.importItems() },
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
        // The find bar lives in the main window; a Flechinha has none.
        {
          label: 'Find…',
          accelerator: k('find'),
          click: () => !flechinhas.focused() && !settings.focusSearch() && browser.openFind(),
        },
        { label: 'Find Next', accelerator: 'CmdOrCtrl+G', click: () => !flechinhas.focused() && findAgain(browser, true) },
        { label: 'Find Previous', accelerator: 'CmdOrCtrl+Shift+G', click: () => !flechinhas.focused() && findAgain(browser, false) },
      ],
    },
    {
      label: 'View',
      submenu: [
        { label: 'Toggle Sidebar', accelerator: k('toggleSidebar'), click: () => browser.toggleSidebar() },
        { type: 'separator' },
        { label: 'Reload', accelerator: k('reload'), click: () => active()?.reload() },
        { label: 'Hard Reload', accelerator: k('hardReload'), click: () => active()?.reloadIgnoringCache() },
        { label: 'Back', accelerator: k('back'), click: () => active()?.navigationHistory.goBack() },
        { label: 'Forward', accelerator: k('forward'), click: () => active()?.navigationHistory.goForward() },
        { type: 'separator' },
        { label: 'Split View with New Tab', accelerator: k('splitNew'), click: () => browser.setPaletteOpen(true, 'split') },
        { label: 'Exit Split View', accelerator: k('exitSplit'), enabled: !!space.split, click: () => browser.exitSplit() },
        { type: 'separator' },
        { label: 'Zoom In', accelerator: k('zoomIn'), click: () => browser.zoom(1) },
        { label: 'Zoom Out', accelerator: k('zoomOut'), click: () => browser.zoom(-1) },
        { label: 'Actual Size', accelerator: k('actualSize'), click: () => browser.zoom(0) },
        { type: 'separator' },
        {
          label: 'Developer',
          submenu: [
            { label: 'Developer Tools', accelerator: k('devTools'), click: () => active()?.toggleDevTools() },
            { label: 'Responsive View', accelerator: k('responsive'), click: () => browser.toggleResponsive() },
            { label: 'Next Environment', accelerator: k('nextEnvironment'), click: () => browser.nextEnvironment() },
            { label: 'Edit Boost for This Site', accelerator: k('editBoost'), click: () => browser.editBoost('script') },
            { type: 'separator' },
            { label: 'View Source', accelerator: k('viewSource'), click: () => tab() && browser.openTab(`view-source:${tab()!.url}`) },
            { label: 'Sidebar Developer Tools', click: () => browser.uiContents.openDevTools({ mode: 'detach' }) },
          ],
        },
        { type: 'separator' },
        { role: 'togglefullscreen' },
      ],
    },
    {
      label: 'Spaces',
      submenu: [
        ...store.state.spaces.map(
          (s, i): MenuItemConstructorOptions => ({
            label: `${s.emoji ? s.emoji + '  ' : ''}${s.name}`,
            type: 'checkbox',
            checked: s.id === space.id,
            accelerator: i < 9 ? `Ctrl+${i + 1}` : undefined,
            click: () => store.switchSpace(s.id),
          }),
        ),
        { type: 'separator' },
        { label: 'Next Space', accelerator: k('nextSpace'), click: () => browser.cycleSpace(1) },
        { label: 'Previous Space', accelerator: k('previousSpace'), click: () => browser.cycleSpace(-1) },
        { type: 'separator' },
        { label: 'New Space', click: () => browser.newSpace() },
        { label: 'Edit Space…', click: () => browser.editSpace(space.id) },
        {
          label: 'Separate Cookies and Logins',
          type: 'checkbox',
          checked: !!space.isolated,
          click: () => browser.setSpaceIsolated(space.id, !space.isolated),
        },
      ],
    },
    {
      label: 'Tabs',
      submenu: [
        { label: 'Next Tab', accelerator: k('nextTab'), click: () => browser.cycleTab(1) },
        { label: 'Previous Tab', accelerator: k('previousTab'), click: () => browser.cycleTab(-1) },
        ...Array.from({ length: 9 }, (_, i) => ({
          label: i === 8 ? 'Last Tab' : `Tab ${i + 1}`,
          accelerator: `CmdOrCtrl+${i + 1}`,
          visible: false,
          acceleratorWorksWhenHidden: true,
          click: () => browser.selectTabIndex(i === 8 ? -1 : i),
        })),
        { type: 'separator' },
        { label: 'Pin / Unpin Tab', accelerator: k('pinTab'), click: () => tab() && store.togglePin(tab()!.id) },
        { label: 'Add to Favorites', click: () => tab() && store.toggleFavorite(tab()!.id) },
        { label: 'Duplicate Tab', accelerator: k('duplicateTab'), click: () => tab() && browser.openTab(tab()!.url) },
        { label: tab()?.muted ? 'Unmute Tab' : 'Mute Tab', accelerator: k('muteTab'), click: () => tab() && browser.toggleMute(tab()!.id) },
        { type: 'separator' },
        { label: 'Clear Today Tabs', accelerator: k('clearToday'), click: () => clearToday(browser, store) },
      ],
    },
    {
      label: 'Archive',
      submenu: [
        { label: 'Recently Closed', enabled: false },
        ...store.state.archive.slice(0, 12).map((item) => ({
          label: item.title.length > 60 ? item.title.slice(0, 57) + '…' : item.title || item.url,
          click: () => browser.openTab(item.url),
        })),
        ...(store.state.archive.length ? [] : [{ label: 'Nothing archived yet', enabled: false }]),
        { type: 'separator' },
        {
          label: 'Archive Today Tabs After',
          submenu: [12, 24, 24 * 7, 24 * 30, 0].map((hours) => ({
            label: hours === 0 ? 'Never' : hours < 24 ? `${hours} Hours` : hours === 24 ? '1 Day' : `${hours / 24} Days`,
            type: 'radio' as const,
            checked: archiveAfter === hours,
            click: () => store.setArchiveAfter(hours),
          })),
        },
      ],
    },
    {
      label: 'Extensions',
      submenu: [
        { label: 'Chrome Web Store', click: () => browser.openTab(WEB_STORE_URL) },
        {
          label: 'Boosts',
          submenu: [
            { label: 'Edit Script for This Site', click: () => browser.editBoost('script') },
            { label: 'Edit Styles for This Site', click: () => browser.editBoost('styles') },
            { type: 'separator' },
            { label: 'Open Boosts Folder', click: () => browser.openBoostsFolder() },
          ],
        },
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
    {
      role: 'help',
      submenu: [
        { label: 'Welcome to Flecha…', click: () => store.setOnboarded(false) },
        { label: 'Keyboard Shortcuts', accelerator: k('shortcuts'), click: () => settings.open('shortcuts') },
        { label: 'Flecha Website', click: () => browser.openTab('https://flecha.oprimo.dev') },
      ],
    },
  ];
  Menu.setApplicationMenu(Menu.buildFromTemplate(template));
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

function registerIpc(browser: Browser, store: Store, history: History, downloads: Downloads, flechinhas: Flechinhas) {
  ipcMain.on('find:query', (event, text: string, forward: boolean, newSearch: boolean) => {
    if (event.sender !== browser.findViewContents) return;
    lastFind = String(text);
    browser.find(lastFind, !!forward, !!newSearch);
  });
  for (const phase of ['start', 'end', 'reset'] as const) {
    ipcMain.on(`divider:${phase}`, (event) => event.sender === browser.dividerContents && browser.dragDivider(phase));
  }
  ipcMain.on('divider:drag', (event, clientX: number) => {
    if (event.sender === browser.dividerContents && Number.isFinite(clientX)) browser.dragDivider(clientX);
  });
  ipcMain.on('find:close', (event) => {
    if (event.sender === browser.findViewContents) browser.closeFind();
  });
  ipcMain.handle('search-history', (event, query: string) =>
    event.sender === browser.uiContents ? history.search(String(query)) : [],
  );
  // Real app icons for the import step: /Applications/<name>.app.
  ipcMain.handle('app-icon', async (event, name: string) => {
    if (event.sender !== browser.uiContents || !/^[\w .-]+$/.test(String(name))) return undefined;
    // app.getFileIcon crashes Electron 45 on macOS; Quick Look's thumbnail of an app is its icon.
    const icon = await nativeImage.createThumbnailFromPath(`/Applications/${name}.app`, { width: 72, height: 72 }).catch(() => undefined);
    return icon && !icon.isEmpty() ? icon.toDataURL() : undefined;
  });
  ipcMain.handle('import:detect', (event) => (event.sender === browser.uiContents ? detectBrowsers() : []));
  ipcMain.handle('import:run', (event, id: string) =>
    event.sender === browser.uiContents ? importBrowser(String(id), store, history) : undefined,
  );
  ipcMain.handle('top-sites', (event) => {
    if (event.sender !== browser.uiContents) return [];
    const sites = history.top();
    browser.favicons?.want(sites.map((site) => site.url));
    return sites;
  });
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
    newSpace: () => browser.newSpace(),
    updateSpace: (id: string, patch: { name?: string; emoji?: string; color?: string }) => store.updateSpace(id, patch ?? {}),
    setSpaceIsolated: (id: string, isolated: boolean) => browser.setSpaceIsolated(id, !!isolated),
    deleteSpace: (id: string) => void browser.deleteSpace(id),
    tabMenu: (id: string) => browser.tabMenu(id),
    spaceMenu: (id: string) => browser.spaceMenu(id),
    toggleFolder: (id: string) => store.toggleFolder(id),
    renameFolder: (id: string, name: string) => store.renameFolder(id, name),
    moveTab: (id: string, target: DropTarget) => store.moveTab(id, target),
    clearToday: () => clearToday(browser, store),
    importMenu: () => void browser.importMenu(),
    openPalette: (mode: PaletteMode) => browser.setPaletteOpen(true, mode),
    closePalette: () => browser.setPaletteOpen(false),
    toggleSidebar: () => browser.toggleSidebar(),
    setSidebarPeek: (peek: boolean) => browser.setSidebarPeek(!!peek),
    renameTab: (id: string, title: string) => store.updateTab(id, { customTitle: String(title).trim() || undefined }),
    toggleMute: (id: string) => browser.toggleMute(id),
    copyText: (text: string) => {
      clipboard.writeText(String(text));
      browser.flash('Link copied');
    },
    capturePage: () => void browser.capturePage(),
    clearSiteData: (origin: string) => void browser.clearSiteData(String(origin)),
    setSitePermission: (origin: string, permission: string, decision: string) =>
      store.setSitePermission(String(origin), String(permission), decision === 'allow' || decision === 'block' ? decision : undefined),
    zoom: (step: number) => browser.zoom(step > 0 ? 1 : step < 0 ? -1 : 0),
    openDevTools: () => browser.activeContents()?.openDevTools({ mode: 'detach' }),
    openSettings: (pane?: string) => browser.openSettings?.(typeof pane === 'string' ? pane : undefined),
    toggleResponsive: () => browser.toggleResponsive(),
    showConsole: (id: string) => browser.showConsole(id),
    editBoost: (kind: BoostKind) => browser.editBoost(kind === 'styles' ? 'styles' : 'script'),
    openBoostsFolder: () => browser.openBoostsFolder(),
    splitWith: (id: string) => store.splitWith(id),
    setTabDragging: (dragging: boolean) => browser.setTabDragging(!!dragging),
    dropToSplit: (id: string, side: string) => browser.dropToSplit(String(id), side === 'left' ? 'left' : 'right'),
    splitNew: (input: string) => browser.splitNew(input),
    closePeek: () => browser.closePeek(),
    openDownload: (id: string) => downloads.open(id),
    revealDownload: (id: string) => downloads.reveal(id),
    cancelDownload: (id: string) => downloads.cancel(id),
    clearDownloads: () => downloads.clear(),
    promotePeek: () => browser.promotePeek(),
    finishOnboarding: () => store.setOnboarded(true),
    setPreferences: async (prefs: { theme?: string; searchEngine?: string; adBlock?: boolean }) => {
      if (prefs?.theme === 'system' || prefs?.theme === 'light' || prefs?.theme === 'dark') {
        store.setPreferences({ theme: prefs.theme });
        nativeTheme.themeSource = prefs.theme;
      }
      if (prefs?.searchEngine && prefs.searchEngine in SEARCH_ENGINES) store.setSearchEngine(prefs.searchEngine as SearchEngineId);
      if (typeof prefs?.adBlock === 'boolean') {
        store.setAdBlock(prefs.adBlock);
        await setAdBlocking(prefs.adBlock).catch((err) => console.error('Ad blocker failed', err));
      }
    },
    makeDefaultBrowser: () => {
      makeDefaultBrowser();
      // macOS confirms in its own dialog; check again once Flecha is back in front.
      browser.win.once('focus', () => browser.sendState());
    },
    newFlechinha: () => flechinhas.create(),
  };
  ipcMain.on('cmd', (event, name: string, ...args: unknown[]) => {
    if (event.sender !== browser.uiContents) return;
    handlers[name]?.(...args);
  });
}

app.whenReady().then(async () => {
  if (!isPrimaryInstance) return;
  const store = new Store(app.getPath('userData'));
  const history = new History(app.getPath('userData'));
  store.archiveStale();
  let browser: Browser | undefined;
  let settings: Settings | undefined;
  // Extensions and the ad blocker load first so they reach the very first tabs.
  const [extensions] = await Promise.all([
    setupExtensions(() => browser),
    setAdBlocking(store.state.adBlock !== false).catch((err) => console.error('Ad blocker failed', err)),
  ]);
  const boosts = new Boosts((file) => browser?.boostChanged(file));
  // Icons arrive in bursts while saved sites are fetched; one sidebar update per burst is enough.
  let faviconTimer: NodeJS.Timeout | undefined;
  const favicons = new Favicons(() => {
    faviconTimer ??= setTimeout(() => {
      faviconTimer = undefined;
      browser?.sendState();
      settings?.tabs.refresh();
    }, 400);
  });
  browser = new Browser(store, history, boosts, extensions);
  const bridge = new AgentBridge(
    {
      listTabs: () => browser!.listTabs(),
      contentsFor: (id) => browser!.contentsFor(id),
      openTab: (url, background) => browser!.openTabFor(url, background),
      activate: (id) => browser!.activate(id),
      close: (id) => browser!.close(id),
      consoleLog: (id) => browser!.consoleLog(id),
      showActivity: (id, action) => browser!.showActivity(id, action),
    },
    () => store.state.agent?.token ?? '',
  );
  nativeTheme.themeSource = store.state.theme ?? 'system';
  if (store.state.agent?.enabled) await bridge.start().catch((err) => console.error('Agent bridge failed', err));
  settings = new Settings(store, () => browser!.sendState(), {
    setEnabled: (enabled) => (enabled ? bridge.start() : bridge.stop()),
    endpoint: () => (bridge.port ? `http://127.0.0.1:${bridge.port}/mcp` : undefined),
  }, () => browser!.openBoostsFolder(), () => {
    store.setOnboarded(false);
    browser!.focus();
  }, {
    loadedContents: (id) => browser!.loadedContents(id),
    destroyView: (id) => browser!.destroyView(id),
    show: (id) => {
      store.activate(id);
      browser!.focus();
    },
    faviconHosts: () => favicons.hosts(),
  });
  const flechinhas = new Flechinhas({
    resolve: (input) => resolveInput(input, store.state.searchEngine),
    searchEngineName: () => searchEngine(store.state.searchEngine).name,
    tint: () => spaceTint(store.activeSpace.color),
    space: () => ({ name: store.activeSpace.name, emoji: store.activeSpace.emoji }),
    openTab: (url, background) => {
      browser!.openTab(url, { activate: !background });
      if (!background) browser!.focus();
    },
    adopt: (view, favicon) => browser!.adoptPage(view, favicon),
    record: (url, title) => history.record(url, title),
    updateTitle: (url, title) => history.updateTitle(url, title),
  });
  store.onChange(() => flechinhas.refresh());
  buildMenu(browser, store, settings, flechinhas);
  browser.openSettings = (pane) => settings.open(pane);
  const rebuildMenu = () => buildMenu(browser!, store, settings, flechinhas);
  void detectBrowsers().then((found) => {
    browser!.otherBrowsers = found;
    rebuildMenu();
  });
  // The Spaces, Tabs and Archive menus show live state; rebuild only when what they show changes.
  let menuSignature = '';
  store.onChange(() => {
    const st = store.state;
    const signature = JSON.stringify([
      st.activeSpaceId,
      st.spaces.map((sp) => [sp.id, sp.name, sp.emoji, sp.isolated, !!sp.split]),
      st.archive.slice(0, 12).map((a) => a.url),
      st.archiveAfterHours,
      store.activeTab?.muted,
      st.keybindings,
    ]);
    if (signature === menuSignature) return;
    menuSignature = signature;
    rebuildMenu();
  });
  session.defaultSession.extensions.on('extension-loaded', rebuildMenu);
  session.defaultSession.extensions.on('extension-unloaded', rebuildMenu);
  const downloadOptions = () => ({
    folder: store.state.downloadsFolder ?? app.getPath('downloads'),
    askWhereToSave: !!store.state.askWhereToSave,
  });
  const downloads = new Downloads(downloadOptions, (items, started) =>
    browser!.uiContents.send('downloads', items, started),
  );
  registerIpc(browser, store, history, downloads, flechinhas);
  onEachSession((ses) => {
    addAdBlockSession(ses);
    downloads.attach(ses);
    registerPermissions(
      ses,
      store,
      (wc) => flechinhas.windowFor(wc) ?? browser!.win,
      (wc) => wc === browser!.uiContents || flechinhas.isToolbar(wc),
    );
  });
  focusBrowser = () => browser!.focus();
  browser.favicons = favicons;
  const wantFavicons = () => favicons.want(Object.values(store.state.tabs).filter((t) => !t.favicon).map((t) => t.url));
  wantFavicons();
  store.onChange(wantFavicons);
  browser.watchLocalServers();
  // Links from other apps get a Flechinha of their own, unless that's turned off in Settings.
  deliverLinksTo((url) => {
    if (store.state.openLinksInFlechinha !== false) return void flechinhas.create(url);
    browser!.openTab(url);
    browser!.focus();
  });

  setInterval(() => {
    for (const id of store.archiveStale()) browser.destroyView(id);
  }, 60 * 60 * 1000);
  app.on('before-quit', () => {
    browser!.snapshotTabs();
    store.saveNow();
    history.saveNow();
  });
});
