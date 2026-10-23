/**
 * chrome.flechaPrivate: what Flecha's browser (the native patches) gives the shell, and only
 * the shell. It is how the shell draws the whole window like the Electron spike did: the
 * frame page (window.html) is a transparent view over the window, and the browser lays the
 * page area and the traffic lights out around it.
 *
 * Absent in stock Chromium; the shell then falls back to the side panel.
 *
 * Layout, the same numbers as the Electron spike's Browser.layout() (src/main/main.ts):
 *  - sidebar 'docked':   frame view = left strip SIDEBAR (260) wide; page area = x 260, y 8,
 *                        to 8 from the right and bottom edges. Traffic lights shown at (16, 18).
 *  - sidebar 'hidden':   frame view = left strip EDGE (8) wide, for the hover edge; page area
 *                        x 8. Traffic lights hidden.
 *  - sidebar 'floating': frame view = FLOAT_MARGIN + SIDEBAR + FLOAT_SHADOW (10 + 260 + 28)
 *                        wide, over the page; page area as 'hidden'. Traffic lights shown at
 *                        (16 + 10, 18 + 10).
 *  - overlay: true:      frame view covers the whole window (command bar, Peek, tour, tab
 *                        drag); the page area doesn't move.
 * The frame view stays above the page; its transparent pixels show the page beneath but
 * still take the mouse within the view's bounds, as in Electron.
 */
declare namespace chrome.flechaPrivate {
  type SidebarMode = 'docked' | 'hidden' | 'floating';

  interface FrameState {
    sidebar: SidebarMode;
    overlay: boolean;
    /** The window's colour behind the page area (the Space tint), as #rrggbb. */
    background: string;
  }

  /** Lays out a window's frame. Applies to every normal window when windowId is omitted. */
  function setFrame(state: FrameState, windowId?: number): Promise<void>;

  /** Moves keyboard focus to the frame page (command bar) or back to the active page. */
  function focus(target: 'frame' | 'page', windowId?: number): Promise<void>;

  /** The window a frame page belongs to; frame pages are opened by the browser, one per window. */
  function getWindowId(): Promise<number>;

  /**
   * Flecha's keyboard shortcuts (patch 0016): `shortcuts` maps Electron accelerators
   * ("CmdOrCtrl+Shift+T", "Alt+CmdOrCtrl+Left", "Ctrl+Tab", "CmdOrCtrl+[") to command ids.
   * Flecha windows take these keys before the page and before Chromium's own shortcuts and
   * menu items (so ⌘S no longer opens Save Page), and send them to onShortcut. Each call
   * replaces the whole map; keys left out go back to Chromium (⌘C, ⌘V, ⌘P…).
   *
   * Syntax: modifiers Cmd/Command/CmdOrCtrl/CommandOrControl/Super/Meta, Ctrl/Control,
   * Alt/Option, Shift, then one key: a letter, a digit, [ ] = - , . / ; ' ` \, Plus, Tab,
   * Space, Enter/Return, Escape/Esc, Backspace, Delete, Up/Down/Left/Right, Home, End,
   * PageUp, PageDown, F1–F24. Resolves with the accelerators it couldn't read (ignored).
   */
  function setShortcuts(shortcuts: Record<string, string>): Promise<string[]>;

  /**
   * The browser's colour scheme (patch 0018), like Electron's nativeTheme.themeSource:
   * Chromium's own UI (menus, dialogs, find bar, traffic lights) and every page's
   * prefers-color-scheme follow it, the frame page's included. 'system' follows macOS. Kept in
   * the profile (Chromium's Settings > Appearance > Mode).
   */
  function setTheme(theme: 'system' | 'light' | 'dark'): Promise<void>;

  /**
   * Split views (patch 0019). In Flecha windows Chromium's split has none of its own chrome:
   * the panes sit straight in the page area, 8px apart (the spike's GAP, which is also the
   * drag handle), each with 10px round corners; no inset, padding, outline or mini toolbar.
   * The left pane is round((width − 8) × ratio) wide, as in Electron's layout().
   *
   * setSplitRatio sets the left pane's share (clamped to 0.1–0.9); `tabId` is either tab of
   * the split. Rejects while the tab isn't in a split yet.
   */
  function setSplitRatio(tabId: number, ratio: number): Promise<void>;

  /** The user finished dragging the gap: `tabId` is the left pane's tab. */
  const onSplitRatioChanged: chrome.events.Event<(tabId: number, ratio: number) => void>;

  /**
   * Flechinha (patch 0023), Flecha's small window for one page, like src/main/flechinha.ts:
   * a normal Chromium window (900×700 at most, centred on the display under the pointer, each
   * new one a step down and right) whose frame page is window.html?window=<id>&kind=flechinha.
   * The frame draws a toolbar FLECHINHA_TOOLBAR (44) tall across the whole width; the page
   * sits under it at x 8, y 44, to 8 from the right and bottom (round corners). Traffic lights
   * at (16, 16). setFrame() with overlay: true makes the frame cover the whole window (for
   * menus); its sidebar mode is ignored. -webkit-app-region: drag moves the window.
   * Links the page opens in new tabs (target=_blank, ⌘-click) open in Flecha's main window;
   * real popups stay popups. Not restored with the session.
   *
   * openFlechinha({ url }) opens a page (default: the new tab page); openFlechinha({ tabId })
   * moves that tab into the new window instead. Resolves with the window id.
   */
  function openFlechinha(options?: { url?: string; tabId?: number }): Promise<number>;

  /**
   * "Open in Flecha": moves the Flechinha's page into Flecha's main window (the last active
   * window that isn't a Flechinha), activates it there and brings that window to the front;
   * the emptied Flechinha closes. Resolves with the tab id (Chromium fires tabs.onDetached /
   * onAttached for it, not onCreated).
   */
  function openInFlecha(windowId: number): Promise<number>;

  /**
   * Whether links from other apps (Flecha as the default browser) open in a new Flechinha,
   * like the spike's state.openLinksInFlechinha. On by default; applies once a Flecha window
   * is up (a link that launches Flecha opens as a normal tab).
   */
  function setOpenLinksInFlechinha(enabled: boolean): Promise<void>;

  /** The open Flechinhas' window ids (e.g. after the background restarts). */
  function getFlechinhas(): Promise<number[]>;

  /** A shortcut from setShortcuts() was pressed in window `windowId`. */
  const onShortcut: chrome.events.Event<(command: string, windowId: number) => void>;
}
