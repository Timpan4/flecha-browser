/**
 * How wide you can drag the sidebar: Chromium 158's limits for its own vertical tab strip,
 * kVerticalTabStripUncollapsedMinWidth and kVerticalTabStripUncollapsedMaxWidth in
 * chrome/browser/ui/tabs/vertical_tab_strip_state.h. The browser keeps setFrame()'s width
 * within the same (patch 0061). The default is Flecha's own (kFlechaSidebarWidth, patch 0010).
 */
export const SIDEBAR_MIN_WIDTH = 126;
export const SIDEBAR_MAX_WIDTH = 400;
export const SIDEBAR_DEFAULT_WIDTH = 260;

export function clampSidebarWidth(width: number) {
  return Math.round(Math.min(SIDEBAR_MAX_WIDTH, Math.max(SIDEBAR_MIN_WIDTH, width)));
}
