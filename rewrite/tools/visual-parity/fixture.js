// Stands in for the shipping shell's hub while shipping.mjs renders its pages headless: the
// same fixture the rewrite's `verify-slice.sh visual` mode builds, handed to the shipping UI
// code through its own APIs (chromium/shared/types.d.ts). Loaded before the page's bundle.
(() => {
  const params = new URLSearchParams(location.search);
  const surface = params.get('surface') ?? 'sidebar';
  const dark = params.get('theme') === 'dark';
  // The rewrite's default Space tint in each theme, as its capture shows it.
  const tint = params.get('tint') ?? (dark ? '#2f3746' : '#cdd9f7');
  const origin = 'http://127.0.0.1:36811';
  const page = (path, extra = {}) => ({ id: path, url: `${origin}/${path}`, title: `Flecha fixture /${path}`, lastActive: 0, ...extra });

  // Every chrome.* property and call result is a do-nothing object, so the shipping modules'
  // listeners and window queries go nowhere. A call's result can be awaited or chained; it
  // settles to the same object without `then`, so awaiting it ends.
  const stub = (thenable) => {
    const self = new Proxy(function () {}, {
      get: (_, key) => {
        if (key === Symbol.toPrimitive) return () => '';
        if (key === 'then') return thenable ? (resolve) => resolve(settled) : undefined;
        return self;
      },
      apply: () => inert,
    });
    return self;
  };
  const inert = stub(true);
  const settled = stub(false);
  // incognito.ts reads this flag directly; the fixture is a normal window.
  const chrome = new Proxy({ extension: { inIncognitoContext: false } }, { get: (target, key) => target[key] ?? inert[key] });
  Object.defineProperty(window, 'chrome', { value: chrome, configurable: true, writable: true });

  const listeners = {};
  const api = (extra = {}) =>
    new Proxy(extra, {
      get(target, key) {
        if (key in target) return target[key];
        if (typeof key === 'string' && key.startsWith('on')) return (cb) => (listeners[key] = cb);
        return () => Promise.resolve([]);
      },
    });
  const emit = (name, ...args) => listeners[name]?.(...args);

  const split = surface === 'split';
  const tabs = Object.fromEntries(
    [page('opener'), page('peek'), page('split'), page('open'), page('favorite')].map((tab) => [tab.id, tab]),
  );
  const personal = {
    id: 'personal',
    name: 'Personal',
    emoji: '🏠',
    color: '#4c7bd9',
    pinned: [
      { kind: 'folder', id: 'reading', name: 'Reading', open: true, children: [{ kind: 'tab', tabId: 'open' }] },
      { kind: 'tab', tabId: 'peek' },
    ],
    today: split ? ['opener', 'split'] : ['opener'],
    activeTabId: 'opener',
    ...(split ? { split: ['opener', 'split'] } : {}),
  };
  const work = { id: 'work', name: 'Work', emoji: '💼', color: '#d9534c', pinned: [], today: [] };
  const state = {
    windowId: 'window',
    spaces: [personal, work],
    activeSpaceId: 'personal',
    favorites: ['favorite'],
    tabs,
    archive: [],
    sidebarVisible: true,
    sidebarWidth: 260,
    nav: { canGoBack: false, canGoForward: false, zoom: surface === 'zoom' ? 110 : 100 },
    searchEngineName: 'Google',
    tint,
    sidebarMode: 'docked',
    localServers: [],
    consoleErrors: {},
    boostedHosts: [],
    faviconHosts: [],
    shortcuts: {},
    onboarded: true,
    adBlock: true,
  };

  window.flecha = api({ ready: () => {} });
  window.flechinha = api({ ready: () => {} });
  const settingsView = {
    adBlock: true,
    searchEngine: 'google',
    searchEngines: ['Google', 'DuckDuckGo', 'Bing', 'Brave Search', 'Kagi', 'Ecosia'].map((name) => ({ id: name.toLowerCase().replace(' ', ''), name })),
    downloadsFolder: '~/Downloads',
    askWhereToSave: false,
    defaultBrowser: { isDefault: false, canSet: true },
    environments: [],
    agent: { enabled: false },
    theme: dark ? 'dark' : 'light',
    archiveAfterHours: 12,
    peekPinnedLinks: true,
    shortcuts: [],
    openLinksInFlechinha: true,
    sidebarInPageFullscreen: false,
  };
  window.settings = api({ get: () => Promise.resolve(settingsView), set: () => Promise.resolve(settingsView) });

  const shellMenu = () => {
    const menu = document.createElement('div');
    menu.className = 'shell-menu';
    menu.style.left = '380px';
    menu.style.top = '200px';
    for (const [label, disabled] of [['Back', true], ['Forward', true], ['Reload', false], ['Inspect element', false]]) {
      const item = document.createElement('button');
      item.className = 'shell-menu-item';
      item.textContent = label;
      item.disabled = disabled;
      menu.append(item);
    }
    document.body.append(menu);
  };

  // The bundle registers its listeners when it runs, before load.
  addEventListener('load', () => {
    if (location.pathname.endsWith('flechinha.html')) {
      emit('onState', {
        url: `${origin}/opener`,
        title: 'Flecha fixture /opener',
        canGoBack: false,
        canGoForward: false,
        loading: false,
        tint,
        space: { name: 'Personal', emoji: '🏠' },
        searchEngineName: 'Google',
      });
      return;
    }
    if (location.pathname.endsWith('settings.html')) return;
    emit('onState', state);
    emit('onDownloads', []);
    if (surface === 'command-bar') emit('onPalette', 'navigate');
    if (surface === 'downloads') document.getElementById('downloads-panel').hidden = false;
    if (surface === 'site-controls' || surface === 'zoom') document.getElementById('btn-site').click();
    if (surface === 'peek') emit('onPeek', { url: `${origin}/peek`, title: 'Flecha fixture /peek', bounds: { x: 300, y: 40, width: 860, height: 960 } });
    if (surface === 'context-menu') shellMenu();
  });
})();
