// Use synthetic browser files, including SQLite, and prove imports leave originals intact.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { loadModule } from './load-module.mjs';

const { detectBrowsers, readBrowser } = await loadModule('chromium/native-host/browserImport.ts');

test('Linux discovers XDG Chromium and Firefox data and imports without changing source files', async () => {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-import-test-'));
  const saved = Object.fromEntries(['HOME', 'USERPROFILE', 'XDG_CONFIG_HOME', 'CHROME_CONFIG_HOME', 'HELIUM_CONFIG_HOME'].map((key) => [key, process.env[key]]));
  try {
    process.env.HOME = home;
    process.env.USERPROFILE = home;
    process.env.XDG_CONFIG_HOME = path.join(home, 'config');
    delete process.env.CHROME_CONFIG_HOME;
    delete process.env.HELIUM_CONFIG_HOME;
    const chrome = path.join(process.env.XDG_CONFIG_HOME, 'google-chrome', 'Default');
    const firefox = path.join(home, '.mozilla', 'firefox', 'test.default-release');
    await fs.mkdir(chrome, { recursive: true });
    await fs.mkdir(firefox, { recursive: true });
    const bookmarks = path.join(chrome, 'Bookmarks');
    await fs.writeFile(bookmarks, JSON.stringify({ roots: { bookmark_bar: { children: [{ type: 'url', name: 'Example', url: 'https://example.com/' }] } } }));
    const history = path.join(chrome, 'History');
    const db = new DatabaseSync(history);
    db.exec("CREATE TABLE urls (url TEXT, title TEXT, visit_count INTEGER, last_visit_time INTEGER, hidden INTEGER); INSERT INTO urls VALUES ('https://example.com/', 'Example', 3, 13300000000000000, 0)");
    db.close();
    const places = path.join(firefox, 'places.sqlite');
    const ff = new DatabaseSync(places);
    ff.exec("CREATE TABLE moz_places (id INTEGER, url TEXT, title TEXT, visit_count INTEGER, last_visit_date INTEGER, hidden INTEGER); CREATE TABLE moz_bookmarks (id INTEGER, parent INTEGER, type INTEGER, title TEXT, guid TEXT, fk INTEGER, position INTEGER); INSERT INTO moz_places VALUES (1, 'https://mozilla.org/', 'Mozilla', 2, 1700000000000000, 0); INSERT INTO moz_bookmarks VALUES (1, 0, 2, 'Toolbar', 'toolbar_____', NULL, 0), (2, 1, 1, 'Mozilla', 'bookmark____', 1, 0)");
    ff.close();
    const original = await Promise.all([bookmarks, history, places].map((file) => fs.readFile(file)));
    assert.deepEqual((await detectBrowsers('linux')).map(({ id }) => id).sort(), ['chrome', 'firefox']);
    const imported = await readBrowser('chrome', 'linux');
    assert.equal(imported.bookmarkCount, 1);
    assert.equal(imported.bookmarks[0].url, 'https://example.com/');
    assert.equal(imported.history[0].visits, 3);
    assert.deepEqual(imported.warnings, []);
    const mozilla = await readBrowser('firefox', 'linux');
    assert.equal(mozilla.bookmarks[0].url, 'https://mozilla.org/');
    assert.equal(mozilla.history[0].lastVisit, 1700000000000);
    assert.deepEqual(mozilla.warnings, []);
    assert.deepEqual(await Promise.all([bookmarks, history, places].map((file) => fs.readFile(file))), original);
    assert.equal((await readBrowser('safari', 'linux')).bookmarkCount, 0);
    process.env.CHROME_CONFIG_HOME = path.join(home, 'chrome-config');
    assert.deepEqual((await detectBrowsers('linux')).map(({ id }) => id), ['firefox']);
    process.env.CHROME_CONFIG_HOME = 'relative-path';
    assert.equal((await detectBrowsers('linux')).some(({ id }) => id === 'chrome'), true);
  } finally {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    await fs.rm(home, { recursive: true, force: true });
  }
});

test('Linux discovers Helium history without bookmarks and respects its independent config override', async () => {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-helium-import-'));
  const saved = Object.fromEntries(['HOME', 'USERPROFILE', 'XDG_CONFIG_HOME', 'CHROME_CONFIG_HOME', 'HELIUM_CONFIG_HOME'].map((key) => [key, process.env[key]]));
  try {
    process.env.HOME = home;
    process.env.USERPROFILE = home;
    process.env.XDG_CONFIG_HOME = path.join(home, 'config');
    process.env.CHROME_CONFIG_HOME = path.join(home, 'chrome-only-config');
    delete process.env.HELIUM_CONFIG_HOME;
    const root = path.join(process.env.XDG_CONFIG_HOME, 'net.imput.helium');
    await fs.mkdir(path.join(root, 'Default'), { recursive: true });
    const history = path.join(root, 'Default', 'History');
    const db = new DatabaseSync(history);
    db.exec("CREATE TABLE urls (url TEXT, title TEXT, visit_count INTEGER, last_visit_time INTEGER, hidden INTEGER); INSERT INTO urls VALUES ('https://example.org/helium', 'Helium fixture', 2, 13300000000000000, 0)");
    db.close();
    const originalHistory = await fs.readFile(history);
    assert.deepEqual(await detectBrowsers('linux'), [{ id: 'helium', name: 'Helium', profiles: 1, hasBookmarks: false, hasHistory: true }]);
    const imported = await readBrowser('helium', 'linux');
    assert.equal(imported.bookmarkCount, 0);
    assert.equal(imported.history[0].url, 'https://example.org/helium');
    assert.deepEqual(imported.warnings, []);
    assert.deepEqual(await fs.readFile(history), originalHistory);
    process.env.HELIUM_CONFIG_HOME = 'relative-path';
    assert.equal((await detectBrowsers('linux'))[0].id, 'helium');
    process.env.HELIUM_CONFIG_HOME = path.join(home, 'helium-override');
    assert.deepEqual(await detectBrowsers('linux'), []);
    await fs.mkdir(process.env.HELIUM_CONFIG_HOME, { recursive: true });
    const relocated = path.join(process.env.HELIUM_CONFIG_HOME, 'net.imput.helium');
    await fs.rename(root, relocated);
    const bookmarks = path.join(relocated, 'Default', 'Bookmarks');
    await fs.writeFile(bookmarks, JSON.stringify({ roots: { bookmark_bar: { children: [{ type: 'url', name: 'Helium bookmark', url: 'https://example.org/bookmark' }] } } }));
    const originalBookmarks = await fs.readFile(bookmarks);
    assert.equal((await detectBrowsers('linux'))[0].hasBookmarks, true);
    const withBookmarks = await readBrowser('helium', 'linux');
    assert.equal(withBookmarks.bookmarks[0].url, 'https://example.org/bookmark');
    assert.equal(withBookmarks.history[0].url, 'https://example.org/helium');
    assert.deepEqual(withBookmarks.warnings, []);
    assert.deepEqual(await fs.readFile(bookmarks), originalBookmarks);
    assert.deepEqual(await fs.readFile(path.join(relocated, 'Default', 'History')), originalHistory);
  } finally {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    await fs.rm(home, { recursive: true, force: true });
  }
});

test('Arc for Windows is found in its package folder, sidebar and all', async () => {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-arc-windows-'));
  const saved = Object.fromEntries(['HOME', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA'].map((key) => [key, process.env[key]]));
  try {
    process.env.HOME = home;
    process.env.USERPROFILE = home;
    process.env.APPDATA = path.join(home, 'Roaming');
    process.env.LOCALAPPDATA = path.join(home, 'Local');
    assert.equal((await detectBrowsers('win32')).some(({ id }) => id === 'arc'), false);
    // An empty package folder an earlier install left behind, listed first, doesn't hide the real one.
    await fs.mkdir(path.join(process.env.LOCALAPPDATA, 'Packages', 'TheBrowserCompany.Arc_aaa-stale'), { recursive: true });
    const arc = path.join(process.env.LOCALAPPDATA, 'Packages', 'TheBrowserCompany.Arc_ttt1ap7aakyb4', 'LocalCache', 'Local', 'Arc');
    await fs.mkdir(path.join(arc, 'User Data', 'Default'), { recursive: true });
    const sidebar = path.join(arc, 'StorableSidebar.json');
    await fs.writeFile(sidebar, JSON.stringify({ sidebar: { containers: [{}, {
      spaces: ['space', { id: 'space', title: 'Work', containerIDs: ['pinned', 'pinned-list', 'unpinned', 'unpinned-list'] }],
      items: [
        'pinned-list', { id: 'pinned-list', childrenIds: ['tab'] },
        'tab', { id: 'tab', title: 'Docs', data: { tab: { savedURL: 'https://example.com/docs' } } },
        'unpinned-list', { id: 'unpinned-list', childrenIds: [] },
      ],
    }] } }));
    const original = await fs.readFile(sidebar);
    const detected = (await detectBrowsers('win32')).find(({ id }) => id === 'arc');
    assert.deepEqual({ name: detected.name, hasBookmarks: detected.hasBookmarks }, { name: 'Arc', hasBookmarks: true });
    const imported = await readBrowser('arc', 'win32');
    assert.deepEqual(imported.warnings, []);
    assert.equal(imported.arc.spaces[0].name, 'Work');
    assert.deepEqual(imported.arc.spaces[0].pinned, [{ kind: 'tab', url: 'https://example.com/docs', title: 'Docs' }]);
    assert.deepEqual(await fs.readFile(sidebar), original);
  } finally {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    await fs.rm(home, { recursive: true, force: true });
  }
});

test('Windows and macOS select their own profile roots and import the same bookmark format', async () => {
  const home = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-platform-import-'));
  const saved = Object.fromEntries(['HOME', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA'].map((key) => [key, process.env[key]]));
  try {
    process.env.HOME = home;
    process.env.USERPROFILE = home;
    process.env.APPDATA = path.join(home, 'Roaming');
    process.env.LOCALAPPDATA = path.join(home, 'Local');
    const roots = [
      ['win32', path.join(process.env.LOCALAPPDATA, 'Google/Chrome/User Data/Default'), 'chrome'],
      ['win32', path.join(process.env.LOCALAPPDATA, 'BraveSoftware/Brave-Browser/User Data/Default'), 'brave'],
      ['win32', path.join(process.env.LOCALAPPDATA, 'Microsoft/Edge/User Data/Default'), 'edge'],
      ['win32', path.join(process.env.LOCALAPPDATA, 'Vivaldi/User Data/Default'), 'vivaldi'],
      ['win32', path.join(process.env.APPDATA, 'Opera Software/Opera Stable/Default'), 'opera'],
      ['win32', path.join(process.env.APPDATA, 'Opera Software/Opera GX Stable/Default'), 'opera-gx'],
      ['win32', path.join(process.env.LOCALAPPDATA, 'Chromium/User Data/Default'), 'chromium'],
      ['win32', path.join(process.env.LOCALAPPDATA, 'imput/Helium/User Data/Default'), 'helium'],
      ['darwin', path.join(home, 'Library/Application Support/Google/Chrome/Default'), 'chrome'],
      ['darwin', path.join(home, 'Library/Application Support/BraveSoftware/Brave-Browser/Default'), 'brave'],
      ['darwin', path.join(home, 'Library/Application Support/Microsoft Edge/Default'), 'edge'],
      ['darwin', path.join(home, 'Library/Application Support/Vivaldi/Default'), 'vivaldi'],
      ['darwin', path.join(home, 'Library/Application Support/com.operasoftware.Opera'), 'opera'],
      ['darwin', path.join(home, 'Library/Application Support/com.operasoftware.OperaGX'), 'opera-gx'],
      ['darwin', path.join(home, 'Library/Application Support/Chromium/Default'), 'chromium'],
      ['darwin', path.join(home, 'Library/Application Support/net.imput.helium/Default'), 'helium'],
    ];
    for (const [, root] of roots) {
      await fs.mkdir(root, { recursive: true });
      await fs.writeFile(path.join(root, 'Bookmarks'), JSON.stringify({ roots: { bookmark_bar: { children: [{ type: 'url', name: 'Platform fixture', url: 'https://example.org/' }] } } }));
    }
    const ids = ['brave', 'chrome', 'chromium', 'edge', 'helium', 'opera', 'opera-gx', 'vivaldi'];
    assert.deepEqual((await detectBrowsers('win32')).map(({ id }) => id).sort(), ids);
    assert.deepEqual((await detectBrowsers('darwin')).map(({ id }) => id).sort(), ids);
    for (const [platform, , id] of roots) {
      const result = await readBrowser(id, platform);
      assert.equal(result.bookmarks[0].url, 'https://example.org/');
      assert.deepEqual(result.warnings, []);
    }
  } finally {
    for (const [key, value] of Object.entries(saved)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
    await fs.rm(home, { recursive: true, force: true });
  }
});
