import path from 'node:path';
import { app, BaseWindow, clipboard, ipcMain, Menu, nativeTheme, screen, WebContentsView, type MenuItemConstructorOptions, type WebContents } from 'electron';
import type { FlechinhaState } from '../shared/types';
import { followGoogleSignIn } from './googleSignIn';
import { sessionForSpace } from './sessions';

const SIZE = { width: 900, height: 700 };
const TOOLBAR = 44;
const GAP = 8;
/** Level with the toolbar's controls. */
const TRAFFIC_LIGHTS = { x: 16, y: 16 };
/** Each new Flechinha steps down and right of the others, so none sits exactly on another. */
const CASCADE = 26;

/** What a Flechinha needs from the rest of Flecha. */
export interface FlechinhaHost {
  /** Typed text to a URL, like the command bar. */
  resolve(input: string): string;
  searchEngineName(): string;
  /** The active Space's colour as the main window shows it. */
  tint(): string;
  space(): { name: string; emoji?: string };
  /** Links the page opens in new tabs go to the main window. */
  openTab(url: string, background: boolean): void;
  /** "Open in Flecha": the page becomes a tab there. */
  adopt(view: WebContentsView, favicon?: string): void;
  record(url: string, title: string): void;
  updateTitle(url: string, title: string): void;
}

/**
 * Flechinha (little Flecha): a small window of its own for a link from another app or a quick
 * lookup, so neither clutters your Spaces. Its page uses the shared session, so it has the same
 * sign-ins, ad blocking and downloads as tabs; Open in Flecha moves the page into the main window.
 */
class Flechinha {
  readonly win: BaseWindow;
  private toolbar: WebContentsView;
  private page?: WebContentsView;
  private favicon?: string;
  /** A video or game is fullscreen; Esc is the page's, to leave it. */
  private pageFullScreen = false;

  constructor(
    private host: FlechinhaHost,
    bounds: Electron.Rectangle,
    private onClosed: () => void,
  ) {
    this.win = new BaseWindow({
      ...bounds,
      minWidth: 480,
      minHeight: 320,
      titleBarStyle: 'hidden',
      trafficLightPosition: TRAFFIC_LIGHTS,
      fullscreenable: false,
      backgroundColor: host.tint(),
      show: false,
    });
    this.toolbar = new WebContentsView({
      webPreferences: {
        preload: path.join(__dirname, '../preload/flechinha-preload.js'),
        sandbox: true,
        contextIsolation: true,
      },
    });
    this.toolbar.setBackgroundColor('#00000000');
    this.win.contentView.addChildView(this.toolbar);
    this.toolbar.webContents.loadFile(path.join(__dirname, '../renderer/flechinha.html'));
    // Shown once the toolbar has painted, so the window never flashes empty.
    this.toolbar.webContents.once('did-finish-load', () => {
      this.win.show();
      this.win.focus();
      app.focus({ steal: true });
      (this.page ?? this.toolbar).webContents.focus();
    });
    this.win.on('resize', () => this.layout());
    this.win.on('closed', () => {
      this.toolbar.webContents.close();
      this.page?.webContents.close();
      this.page = undefined;
      onClosed();
    });
    this.layout();
  }

  get toolbarContents() {
    return this.toolbar.webContents;
  }

  get pageContents(): WebContents | undefined {
    return this.page?.webContents;
  }

  focus() {
    if (this.win.isMinimized()) this.win.restore();
    this.win.show();
    this.win.focus();
    app.focus({ steal: true });
  }

  close() {
    if (!this.win.isDestroyed()) this.win.close();
  }

  load(input: string) {
    const url = this.host.resolve(input);
    const view = this.page ?? this.createPage();
    view.webContents.loadURL(url);
    view.webContents.focus();
    this.sendState();
  }

  /** ⌘L */
  editAddress() {
    this.toolbar.webContents.focus();
    this.toolbar.webContents.send('flechinha:edit-address');
  }

  copyLink() {
    const url = this.page?.webContents.getURL();
    if (!url) return;
    clipboard.writeText(url);
    this.flash('Link copied');
  }

  /** Hands the page to the main window as a Today tab, then goes away. */
  openInFlecha() {
    const view = this.page;
    if (!view) return;
    this.page = undefined;
    this.win.contentView.removeChildView(view);
    this.host.adopt(view, this.favicon);
    this.close();
  }

  command(name: string, args: unknown[]) {
    const wc = this.page?.webContents;
    switch (name) {
      case 'ready':
        return this.sendState();
      case 'back':
        return wc?.navigationHistory.goBack();
      case 'forward':
        return wc?.navigationHistory.goForward();
      case 'reload':
        return wc?.reload();
      case 'stop':
        return wc?.stop();
      case 'navigate':
        return typeof args[0] === 'string' && args[0].trim() ? this.load(args[0]) : undefined;
      case 'copyLink':
        return this.copyLink();
      case 'openInFlecha':
        return this.openInFlecha();
      case 'close':
        return this.close();
    }
  }

  sendState() {
    if (this.toolbar.webContents.isDestroyed()) return;
    this.win.setBackgroundColor(this.host.tint());
    const wc = this.page?.webContents;
    const state: FlechinhaState = {
      url: wc?.getURL() ?? '',
      title: wc?.getTitle() ?? '',
      favicon: this.favicon,
      canGoBack: wc?.navigationHistory.canGoBack() ?? false,
      canGoForward: wc?.navigationHistory.canGoForward() ?? false,
      loading: wc?.isLoading() ?? false,
      tint: this.host.tint(),
      space: this.host.space(),
      searchEngineName: this.host.searchEngineName(),
    };
    this.toolbar.webContents.send('flechinha:state', state);
  }

  private flash(message: string) {
    this.toolbar.webContents.send('flechinha:flash', message);
  }

  private createPage() {
    // The default session: tabs' sign-ins, ad blocking, permissions and downloads.
    const view = new WebContentsView({ webPreferences: { session: sessionForSpace(undefined), sandbox: true, contextIsolation: true } });
    view.setBorderRadius(10);
    const wc = view.webContents;
    // After Open in Flecha the page belongs to a tab, and these listeners fall silent.
    const mine = () => this.page === view;
    const send = () => mine() && this.sendState();

    wc.on('did-start-loading', send);
    wc.on('did-stop-loading', send);
    wc.on('page-title-updated', (_e, title) => {
      if (!mine()) return;
      this.host.updateTitle(wc.getURL(), title);
      this.sendState();
    });
    wc.on('page-favicon-updated', (_e, favicons) => {
      if (!mine()) return;
      this.favicon = favicons[0];
      this.sendState();
    });
    wc.on('did-start-navigation', (details) => {
      if (mine() && details.isMainFrame && !details.isSameDocument) this.favicon = undefined;
    });
    wc.on('did-navigate', (_e, url) => {
      if (!mine()) return;
      this.host.record(url, wc.getTitle());
      this.sendState();
    });
    wc.on('did-navigate-in-page', (_e, _url, isMainFrame) => isMainFrame && send());
    wc.on('enter-html-full-screen', () => (this.pageFullScreen = true));
    wc.on('leave-html-full-screen', () => (this.pageFullScreen = false));
    wc.on('before-input-event', (event, input) => {
      if (!mine() || input.type !== 'keyDown') return;
      const plain = !input.meta && !input.control && !input.alt && !input.shift;
      if (input.key === 'Escape' && plain && !this.pageFullScreen) {
        event.preventDefault();
        this.close();
      } else if (input.meta && !input.shift && !input.alt && input.key.toLowerCase() === 'o') {
        event.preventDefault();
        this.openInFlecha();
      }
    });
    wc.on('context-menu', (_e, params) => mine() && this.pageMenu(wc, params));
    followGoogleSignIn(wc);
    wc.on('did-create-window', (popup) => followGoogleSignIn(popup.webContents));
    wc.setWindowOpenHandler(({ url, disposition }) => {
      // Real popups (OAuth, payment) need window.opener, so let them open as windows.
      if (disposition === 'new-window') return { action: 'allow', overrideBrowserWindowOptions: { width: 520, height: 720 } };
      // Everything else becomes a tab in Flecha; the Flechinha stays about the one page.
      const background = disposition === 'background-tab';
      this.host.openTab(url, background);
      if (background) this.flash('Opened in Flecha');
      return { action: 'deny' };
    });

    this.page = view;
    this.win.contentView.addChildView(view);
    this.layout();
    return view;
  }

  private pageMenu(wc: WebContents, params: Electron.ContextMenuParams) {
    const items: MenuItemConstructorOptions[] = [];
    if (params.linkURL) {
      items.push(
        { label: 'Open Link in Flecha', click: () => this.host.openTab(params.linkURL, false) },
        { label: 'Open Link Here', click: () => wc.loadURL(params.linkURL) },
        { label: 'Copy Link', click: () => clipboard.writeText(params.linkURL) },
        { type: 'separator' },
      );
    }
    if (params.isEditable) items.push({ role: 'cut' }, { role: 'copy' }, { role: 'paste' }, { type: 'separator' });
    else if (params.selectionText) items.push({ role: 'copy' }, { type: 'separator' });
    items.push(
      { label: 'Back', enabled: wc.navigationHistory.canGoBack(), click: () => wc.navigationHistory.goBack() },
      { label: 'Forward', enabled: wc.navigationHistory.canGoForward(), click: () => wc.navigationHistory.goForward() },
      { label: 'Reload', click: () => wc.reload() },
      { type: 'separator' },
      { label: 'Open in Flecha', click: () => this.openInFlecha() },
      { label: 'Copy Page Link', click: () => this.copyLink() },
      { type: 'separator' },
      { label: 'Inspect Element', click: () => wc.inspectElement(params.x, params.y) },
    );
    Menu.buildFromTemplate(items).popup({ window: this.win });
  }

  private layout() {
    const { width, height } = this.win.getContentBounds();
    // With nothing open yet the toolbar page also draws the empty card below itself.
    this.toolbar.setBounds({ x: 0, y: 0, width, height: this.page ? TOOLBAR : height });
    this.page?.setBounds({ x: GAP, y: TOOLBAR, width: width - GAP * 2, height: height - TOOLBAR - GAP });
  }
}

/** Every open Flechinha; there can be several at once. */
export class Flechinhas {
  private open = new Set<Flechinha>();
  private shown = '';

  constructor(private host: FlechinhaHost) {
    ipcMain.on('flechinha:cmd', (event, name: string, ...args: unknown[]) => {
      this.withToolbar(event.sender)?.command(String(name), args);
    });
    nativeTheme.on('updated', () => this.refresh(true));
  }

  /** Opens a new Flechinha on the display under the cursor, with a page or empty for typing. */
  create(url?: string) {
    const flechinha = new Flechinha(this.host, this.placement(), () => this.open.delete(flechinha));
    this.open.add(flechinha);
    if (url) flechinha.load(url);
    return flechinha;
  }

  /** The Flechinha in front, if the key window is one. */
  focused() {
    const win = BaseWindow.getFocusedWindow();
    return win ? [...this.open].find((f) => f.win === win) : undefined;
  }

  /** Where a page's permission prompts belong. */
  windowFor(wc: WebContents) {
    return [...this.open].find((f) => f.pageContents === wc)?.win;
  }

  isToolbar(wc: WebContents) {
    return !!this.withToolbar(wc);
  }

  /** The Space or theme changed: retint the toolbars (only when what they show changed). */
  refresh(force = false) {
    const signature = JSON.stringify([this.host.tint(), this.host.space(), this.host.searchEngineName()]);
    if (!force && signature === this.shown) return;
    this.shown = signature;
    for (const f of this.open) f.sendState();
  }

  private withToolbar(wc: WebContents) {
    return [...this.open].find((f) => f.toolbarContents === wc);
  }

  private placement(): Electron.Rectangle {
    const display = screen.getDisplayNearestPoint(screen.getCursorScreenPoint());
    const area = display.workArea;
    const width = Math.min(SIZE.width, area.width - 40);
    const height = Math.min(SIZE.height, area.height - 40);
    const others = [...this.open].filter((f) => !f.win.isDestroyed() && screen.getDisplayMatching(f.win.getBounds()).id === display.id).length;
    const room = Math.max(1, Math.floor(Math.min(area.width - width, area.height - height) / 2 / CASCADE));
    const step = (others % room) * CASCADE;
    return {
      x: Math.round(area.x + (area.width - width) / 2 + step),
      y: Math.round(area.y + (area.height - height) / 2 + step),
      width,
      height,
    };
  }
}
