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
}
