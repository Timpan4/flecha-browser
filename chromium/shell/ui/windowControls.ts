/**
 * Flecha's close, minimize and maximize circles (windowControls.css), where a window has no
 * title bar of the platform's: Linux and Windows. The page's #wc-close, #wc-minimize and
 * #wc-maximize act on `windowId`; the root element's data-window-state and
 * data-window-focused say whether that window is maximized and focused.
 */
export function setupWindowControls(windowId: Promise<number>) {
  const root = document.documentElement;
  const $ = (id: string) => document.getElementById(id)!;
  const maximize = $('wc-maximize');
  const showState = (win: chrome.windows.Window) => {
    root.dataset.windowState = win.state ?? 'normal';
    maximize.setAttribute('aria-label', win.state === 'maximized' ? 'Restore' : 'Maximize');
  };
  const showFocused = async (focusedId: number) => (root.dataset.windowFocused = String(focusedId === (await windowId)));
  void windowId.then((id) => chrome.windows.get(id)).then(showState);
  void chrome.windows.getLastFocused().then((win) => showFocused(win.id ?? chrome.windows.WINDOW_ID_NONE));
  chrome.windows.onBoundsChanged.addListener(async (win) => win.id === (await windowId) && showState(win));
  chrome.windows.onFocusChanged.addListener((id) => void showFocused(id));
  $('wc-close').onclick = async () => chrome.windows.remove(await windowId);
  $('wc-minimize').onclick = async () => chrome.windows.update(await windowId, { state: 'minimized' });
  maximize.onclick = async () => {
    const id = await windowId;
    const win = await chrome.windows.get(id);
    showState(await chrome.windows.update(id, { state: win.state === 'maximized' ? 'normal' : 'maximized' }));
  };
}
