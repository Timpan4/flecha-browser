import { mkdir, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { z } from 'zod';
import { arcArchive, arcSidebar, localState } from '../../../../chromium/tests/arc-fixture.mjs';
import { loadModule } from '../../../../chromium/tests/load-module.mjs';
import type { BrowserImportData } from '../../../../chromium/shared/importApply';
import type { BrowserState, PinnedNode } from '../../../../chromium/shared/types';

const { readBookmarksHtml, readBrowser, detectBrowsers }: typeof import('../../../../chromium/native-host/browserImport') =
  await loadModule('chromium/native-host/browserImport.ts');

const { previewBrowserImport, applyBrowserImport }: typeof import('../../../../chromium/shared/importApply') =
  await loadModule('chromium/shared/importApply.ts');

const { decodeState }: typeof import('../../../../chromium/shared/savedState') =
  await loadModule('chromium/shared/savedState.ts');

const { chooseFlechaState }: typeof import('../../../../chromium/shell/lib/flechaImport') =
  await loadModule('chromium/shell/lib/flechaImport.ts');

const { Store }: typeof import('../../../../chromium/shared/store') = await loadModule('chromium/shared/store.ts');

type Database = { path: string; schema: string; wal?: string };

type Profile = { platform: 'linux' | 'darwin' | 'win32'; browser: string; files?: Record<string, string>; databases?: Database[] };

const chromeSchema = 'CREATE TABLE urls (url, title, visit_count, last_visit_time, hidden);';

const chromeHistory = (path: string): Database => ({
  path,
  schema: chromeSchema + [
    "INSERT INTO urls VALUES ('https://example.org/high?fixture=1', 'History title', 3, 11644473600006123, 0)",
    "INSERT INTO urls VALUES ('https://example.org/older', NULL, 0, 11644473600002000, 0)",
    "INSERT INTO urls VALUES ('https://example.org/hidden', 'Hidden', 1, 11644473600009123, 1)",
    "INSERT INTO urls VALUES ('https://example.org/zero', 'Zero', 1, 0, 0)",
    "INSERT INTO urls VALUES ('file:///synthetic', 'Non-web history', 2, 11644473600001000, 0)",
  ].join(';') + ';',
  wal: "INSERT INTO urls VALUES ('https://example.org/recent', 'WAL title', '4', 11644473600007000, 0);",
});

type ChromeTab = { type: 'url'; url: string | boolean; name: string | number };

type ChromeFolder = { type: 'folder'; name: string; children: ChromeBookmark[] };

type ChromeBookmark = ChromeTab | ChromeFolder;

const chromeTab = (url: string, name: ChromeTab['name']): ChromeTab => ({ type: 'url', url, name });
const chromeFolder = (name: string, children: ChromeBookmark[]): ChromeFolder => ({ type: 'folder', name, children });

const bookmarks = (bar: ChromeBookmark[], other: ChromeBookmark[] = [], mobile: ChromeBookmark[] = []) => JSON.stringify({
  roots: { bookmark_bar: { children: bar }, other: { children: other }, synced: { children: mobile } },
});

const firefoxHistory: Database = {
  path: '.mozilla/firefox/a.default-release/places.sqlite',
  schema: [
    'CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url, title, visit_count, last_visit_date, hidden);',
    'CREATE TABLE moz_bookmarks (id INTEGER PRIMARY KEY, parent, type, title, guid, fk, position);',
    "INSERT INTO moz_places VALUES (1, 'https://example.org/toolbar', 'Toolbar visit', 5, 20000123, 0),",
    "(2, 'https://example.org/menu', NULL, 1, 19000999, 0), (3, 'https://example.org/mobile', 'Hidden visit', 1, 21000000, 1),",
    "(4, 'https://example.org/tag', 'No visits', 0, NULL, 0);",
    "INSERT INTO moz_bookmarks VALUES (100, 0, 2, 'Toolbar', 'toolbar_____', NULL, 0),",
    "(101, 0, 2, 'Menu', 'menu________', NULL, 1), (102, 0, 2, 'Unsorted', 'unfiled_____', NULL, 2),",
    "(103, 0, 2, 'Mobile', 'mobile______', NULL, 3), (105, 0, 2, 'Tags', 'tags________', NULL, 4),",
    "(1, 100, 2, 'Folder', 'folder______', NULL, 1), (2, 1, 1, 'Toolbar bookmark', 'one_________', 1, 0),",
    "(3, 101, 1, NULL, 'two_________', 2, 0), (4, 103, 1, 'Mobile bookmark', 'three_______', 3, 0),",
    "(5, 100, 3, NULL, 'separator___', NULL, 0), (6, 105, 1, 'Ignored tag', 'ignored_____', 4, 0);",
  ].join('\n'),
};

const arcFiles = (base: string, sidebar = arcSidebar()) => ({
  [base + '/StorableSidebar.json']: JSON.stringify(sidebar),
  [base + '/StorableArchiveItems.json']: JSON.stringify(arcArchive()),
  [base + '/User Data/Local State']: JSON.stringify(localState),
});

const malformedArc = arcSidebar();
const arcMain = malformedArc.sidebar.containers.find((container) => 'items' in container);

if (!arcMain?.items) throw new Error('The Arc fixture has no items');

arcMain.items.push(42, 'childless', { id: 'childless', data: { tab: { savedURL: null } } });

const pinnedContainer = z.looseObject({ id: z.literal('p-pinned'), childrenIds: z.array(z.string()) });

arcMain.items = arcMain.items.map((item) => {
  const parsed = pinnedContainer.safeParse(item);

  return parsed.success ? { ...parsed.data, childrenIds: [...parsed.data.childrenIds, 'childless'] } : item;
});

const windowsArc = 'local/Packages/TheBrowserCompany.Arc_fixture/LocalCache/Local/Arc';
const macArc = 'Library/Application Support/Arc';

const cases: Array<{ name: string; profile: Profile; starterName?: string; rust?: { reason: string; dataWarnings?: string[]; omitHistoryUrl?: string; repairTitles?: boolean } }> = [
  { name: 'chrome-linux', profile: { platform: 'linux', browser: 'chrome', files: {
    'chrome-config/google-chrome/Local State': JSON.stringify({ profile: { info_cache: { Default: { name: 'First' }, 'Profile 2': { name: 'Second' } } } }),
    'chrome-config/google-chrome/Default/Bookmarks': bookmarks([
      chromeTab('https://example.org/bar', 'Toolbar'),
      chromeFolder('Folder', [chromeTab('https://example.org/folder', 'Nested'), chromeTab('chrome://settings', 'Internal')]),
    ], [chromeTab('https://example.org/other', '')]),
    'chrome-config/google-chrome/Default/AccountBookmarks': bookmarks([chromeTab('https://example.org/bar', 'Duplicate')], [], [
      chromeTab('HTTPS://example.org/mobile', 12),
    ]),
    'chrome-config/google-chrome/Profile 2/Bookmarks': bookmarks([
      chromeTab('https://example.org/bar', 'Duplicate profile'), chromeTab('https://example.org/second', 'Second profile'),
    ]),
  }, databases: [chromeHistory('chrome-config/google-chrome/Default/History')] } },
  { name: 'helium-linux', profile: { platform: 'linux', browser: 'helium', databases: [chromeHistory('helium-config/net.imput.helium/History')] } },
  { name: 'firefox-linux', profile: { platform: 'linux', browser: 'firefox', databases: [firefoxHistory] } },
  { name: 'chrome-macos', profile: { platform: 'darwin', browser: 'chrome', files: {
    'Library/Application Support/Google/Chrome/Default/Bookmarks': bookmarks([chromeTab('https://example.org/mac', '')]),
  } } },
  { name: 'chrome-json-surrogates', profile: { platform: 'linux', browser: 'chrome', files: {
    'chrome-config/google-chrome/Default/Bookmarks': bookmarks([
      chromeTab('https://example.org/title', '\uD800 broken \uDC00 pair \uD83D\uDE42 literal \\ud800'),
      chromeTab('https://example.org/next', 'Intact'),
    ]),
  } }, rust: { reason: 'Rust strings replace each isolated surrogate with U+FFFD and preserve valid pairs and escaped literal backslashes.',
    repairTitles: true } },
  { name: 'opera-gx-windows', profile: { platform: 'win32', browser: 'opera-gx', files: {
    'roaming/Opera Software/Opera GX Stable/Bookmarks': bookmarks([chromeTab('https://example.org/opera', 'Opera')]),
  } } },
  { name: 'arc-windows', profile: { platform: 'win32', browser: 'arc', files: {
    'local/Packages/TheBrowserCompany.Arc_empty/LocalCache/Local/Arc/placeholder': '',
    ...arcFiles(windowsArc),
  } } },
  { name: 'arc-macos', profile: { platform: 'darwin', browser: 'arc', files: arcFiles(macArc),
    databases: [chromeHistory(macArc + '/User Data/Default/History')] } },
  { name: 'arc-js-space-matching', starterName: 'personal', profile: { platform: 'darwin', browser: 'arc', files: {
    [macArc + '/StorableSidebar.json']: JSON.stringify({ sidebar: { containers: [{
      spaces: [
        { id: 'joins', title: '\uFEFFPersonal\uFEFF', containerIDs: ['pinned', 'root'] },
        { id: 'stays-separate', title: '\u0085Personal\u0085', containerIDs: ['pinned', 'root'] },
      ],
      items: [
        { id: 'root', childrenIds: ['tab'] },
        { id: 'tab', data: { tab: { savedURL: 'https://example.org/space-matching' } } },
      ],
    }] } }),
  } } },
  { name: 'arc-malformed-record', profile: { platform: 'darwin', browser: 'arc', files: arcFiles(macArc, malformedArc) },
    rust: { reason: 'The type-mismatched Arc item is skipped on both sides. Rust also reports the malformed record.',
      dataWarnings: ['Skipped or repaired 1 malformed Arc sidebar records.'] } },
  { name: 'malformed-browser-rows', profile: { platform: 'linux', browser: 'chrome', files: {
    'chrome-config/google-chrome/Default/Bookmarks': bookmarks([
      { type: 'url', url: false, name: 'Unusable' }, chromeTab('https://example.org/intact', 'Intact'),
    ]),
  }, databases: [{
    path: 'chrome-config/google-chrome/Default/History',
    schema: chromeSchema + "INSERT INTO urls VALUES (123, 'Wrong URL type', 1, 11644473600001000, 0), ('https://example.org/intact', 'Intact', 2, 11644473600002000, 0);",
  }] }, rust: { reason: 'Rust skips the non-text URL instead of making an address out of a number, and reports the dropped records.',
    omitHistoryUrl: '123', dataWarnings: ['Skipped 1 malformed bookmark records.', 'Skipped 1 malformed history records.'] } },
  { name: 'safari-history', profile: { platform: 'darwin', browser: 'safari', databases: [{
    path: 'Library/Safari/History.db',
    schema: [
      'CREATE TABLE history_items (id INTEGER PRIMARY KEY, url, visit_count);',
      'CREATE TABLE history_visits (history_item, title, visit_time);',
      "INSERT INTO history_items VALUES (1, 'https://example.org/safari', 2);",
      "INSERT INTO history_visits VALUES (1, 'Older title', 100), (1, 'Newer title', 200.25);",
    ].join('\n'),
  }] } },
];

function projectState(state: BrowserState) {
  const tab = (id: string) => ({
    url: state.tabs[id].url,
    title: state.tabs[id].title,
    homeUrl: state.tabs[id].homeUrl ?? null,
  });

  const tree = (nodes: PinnedNode[]): unknown[] => nodes.map((node) =>
    node.kind === 'tab' ? tab(node.tabId) : { name: node.name, children: tree(node.children) });

  const win = state.windows[0];

  return {
    spaces: state.spaces.map((space) => ({
      name: space.name,
      emoji: space.emoji ?? null,
      color: space.color,
      isolated: space.isolated ?? false,
      pinned: tree(space.pinned),
      today: (win.spaces[space.id]?.today ?? []).map(tab),
      split: (win.spaces[space.id]?.split ?? []).map((id) => state.tabs[id].url),
    })),
    activeSpace: state.spaces.find((space) => space.id === win.activeSpaceId)?.name,
    favorites: state.favorites.map(tab),
    archive: state.archive,
  };
}

async function expected(data: BrowserImportData, starterName = 'Personal') {
  const store = new Store({ load: () => undefined, save: () => {} });
  store.state.spaces[0].name = starterName;
  const preview = previewBrowserImport(data, store.state, true);
  const history = { merge: (visits: BrowserImportData['history']) => ({ added: visits.length, failed: 0 }) };
  const options = { mode: 'merge' as const, isolate: [1], spacesKey: preview.spacesKey };
  const stale = await applyBrowserImport(data, { ...options, spacesKey: 'stale' }, store, history);
  const merge = await applyBrowserImport(data, options, store, history);
  const mergedState = projectState(store.state);
  const again = await applyBrowserImport(data, options, store, history);
  const append = await applyBrowserImport(data, { ...options, mode: 'append' }, store, history);
  const appendedState = projectState(store.state);

  return { data, preview, stale, merge, mergedState, again, append, appendedState };
}

const output: unknown[] = [];
const home = await mkdtemp(join(tmpdir(), 'flecha-import-fixtures-'));
const env = { ...process.env };

try {
  for (const { name, profile, rust, starterName } of cases) {
    const root = join(home, name);
    await mkdir(root);
    Object.assign(process.env, {
      HOME: root,
      USERPROFILE: root,
      XDG_CONFIG_HOME: join(root, 'config'),
      CHROME_CONFIG_HOME: join(root, 'chrome-config'),
      HELIUM_CONFIG_HOME: join(root, 'helium-config'),
      LOCALAPPDATA: join(root, 'local'),
      APPDATA: join(root, 'roaming'),
    });
    const databases: DatabaseSync[] = [];

    try {
      for (const [file, text] of Object.entries(profile.files ?? {})) {
        await mkdir(dirname(join(root, file)), { recursive: true });
        await writeFile(join(root, file), text);
      }

      for (const database of profile.databases ?? []) {
        const file = join(root, database.path);
        await mkdir(dirname(file), { recursive: true });
        const db = new DatabaseSync(file);
        databases.push(db);
        db.exec(database.schema);

        if (database.wal) {
          db.exec('PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;');
          db.exec(database.wal);
        }
      }

      const found = await detectBrowsers(profile.platform);
      const data = await readBrowser(profile.browser, profile.platform);
      const ts = await expected(data, starterName);

      const override = rust && {
        reason: rust.reason,
        expected: await expected({
          ...data,
          bookmarks: rust.repairTitles ? data.bookmarks?.map((node) => node.kind === 'tab' ? { ...node, title: node.title.toWellFormed() } : node) : data.bookmarks,
          history: data.history.filter((row) => row.url !== rust.omitHistoryUrl),
          warnings: [...data.warnings, ...(rust.dataWarnings ?? [])],
        }, starterName),
      };

      output.push({ name, profile, found, starterName, ts, rust: override });
    } finally {
      for (const db of databases) db.close();
    }
  }
} finally {
  process.env = env;
  await rm(home, { recursive: true });
}

const htmlFixture = 'chromium/native-host/fixtures/bookmarks.html';

output.push({ name: 'committed-bookmarks-html', htmlFixture, ts: await expected(readBookmarksHtml(await readFile(htmlFixture, 'utf8'))) });

const html = '<DL><H3 PERSONAL_TOOLBAR_FOLDER\uFEFF=\uFEFFtrue>Toolbar</H3><DL>'
  + '<A HREF\uFEFF=\uFEFFhttps://example.org/a?a=1&amp;b=2>\uFEFFAlpha\u0085Beta&nbsp;\u3000Gamma</A>'
  + '<A HREF="https://example.org/b">&#xd800; &#x1f642; &#xD83D;&#xDE42;</A>'
  + '<A HREF="https://example.org/c">' + '\u{1f642}'.repeat(80) + '</A></DL></DL>';

output.push({ name: 'bookmarks-js-text', html, ts: await expected(readBookmarksHtml(html)),
  rust: { reason: 'Replace the isolated UTF-16 surrogate from the numeric entity with U+FFFD. JS whitespace and complete supplementary characters keep their original behavior.',
    expected: await expected(readBookmarksHtml(html.replace('&#xd800;', '&#xfffd;'))) } });

const exportText = JSON.stringify({ version: 1, spaces: [{
  id: 'saved-space', name: 'Saved export', emoji: '\u{1F9EA}', color: '#ccffee',
  pinned: [{ kind: 'folder', id: 'folder', name: 'Work', open: true, children: [{ kind: 'tab', tabId: 'pin' }] }],
  today: ['today', 'missing'], split: ['pin', 'today'],
}], tabs: {
  pin: { url: 'https://example.org/export-pin', title: 'Pinned', homeUrl: 'https://example.org/export-pin' },
  today: { url: 'https://example.org/export-today', title: 'Open' },
  broken: { url: false, title: 'Bad record' },
}, archive: [{ url: 'https://example.org/archived', title: 'Archived', closedAt: 1 }] });

const accepted = await chooseFlechaState({ readNative: async () => undefined, pickFile: async () => exportText, confirm: () => true });
const decoded = decodeState(accepted);

if (!decoded) throw new Error('The synthetic Flecha export did not decode');

output.push({ name: 'flecha-export', exportText, ts: projectState(decoded) });

await writeFile(new URL('../tests/fixtures/import-parity.jsonl', import.meta.url),
  output.map((entry) => JSON.stringify(entry)).join('\n') + '\n');
