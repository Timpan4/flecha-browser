// The preview of an Arc import and what applying it adds, from the synthetic fixture (arc-fixture.mjs).
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';
import { arcArchive, arcSidebar } from './arc-fixture.mjs';

const { parseArcSidebar, parseArcArchive } = await loadModule('chromium/native-host/arcImport.ts');
const { previewBrowserImport, applyBrowserImport } = await loadModule('chromium/shared/importApply.ts');
const { Store } = await loadModule('chromium/shared/store.ts');

const names = { 'Profile 1': 'Work profile' };
const arcData = () => {
  const arc = parseArcSidebar(arcSidebar(), names);
  arc.archive = parseArcArchive(arcArchive());
  return { name: 'Arc', arc, bookmarkCount: 0, history: [{ url: 'https://seen.example.com/', title: 'Seen', visits: 1, lastVisit: 1 }], warnings: [] };
};
const noHistory = { merge: (entries) => entries.length };

test('the preview lists each Space with its tabs, whether Merge joins it, its Profile, and what stays behind', () => {
  const store = new Store({ load: () => undefined, save: () => {} });
  store.updateSpace(store.state.spaces[0].id, { name: 'Work' });
  const preview = previewBrowserImport(arcData(), store.state, true);
  assert.deepEqual(preview.spaces, [
    { name: 'Personal', emoji: '🏠', color: '#336699', pinned: 5, today: 4, splits: 2, matches: false, profile: undefined },
    { name: 'Work', emoji: '💼', color: '#ff0080', pinned: 1, today: 4, splits: 2, matches: true, profile: 'Work profile' },
  ]);
  assert.deepEqual([preview.favorites, preview.archived, preview.history, preview.canIsolate], [2, 2, 1, true]);
  assert.deepEqual(preview.skipped.slice(0, 4), [
    '1 tab with an address other than a web page (such as arc:)',
    '1 sidebar item of a kind Flecha doesn’t have (Arc calls it “easel”)',
    // Personal's three-tab split, and Work's second split.
    '2 split views kept as tabs side by side: Flecha shows one split of two tabs per Space',
    'Arc Profiles (“Work profile”): Flecha has no Profiles yet. Their Spaces can have Separate Cookies and Logins instead',
  ]);
  assert.match(preview.skipped.join('\n'), /Little Arc/);
  assert.match(preview.skipped.join('\n'), /Arc Notes aren’t in any file Flecha can read/);
});

test('importing merges by name, maps colour, split, archive and isolation, and summarizes Profiles and Notes', async () => {
  const store = new Store({ load: () => undefined, save: () => {} });
  const result = await applyBrowserImport(arcData(), { mode: 'merge', isolate: [1] }, store, noHistory);
  assert.deepEqual(
    { spaces: result.spaces, merged: result.merged, bookmarks: result.bookmarks, archived: result.archived, history: result.history },
    // 5 + 4 Personal, 1 + 4 Work, 2 favorites.
    { spaces: 2, merged: 0, bookmarks: 16, archived: 2, history: 1 },
  );
  const [personal, work] = store.state.spaces;
  assert.deepEqual([personal.color, personal.isolated, work.color, work.isolated], ['#336699', undefined, '#ff0080', true]);
  const url = (id) => store.state.tabs[id].url;
  assert.deepEqual(store.view(personal).split.map(url), ['https://left.example.com/', 'https://right.example.com/']);
  assert.deepEqual(store.view(work).split.map(url), ['https://w1.example.com/', 'https://w2.example.com/']);
  assert.deepEqual(store.state.archive.map((a) => [a.url, a.title, new Date(a.closedAt).toISOString()]), [
    ['https://old.example.com/', 'Old, renamed', '2025-06-01T10:00:00.000Z'],
    ['https://older.example.com/', 'Older', '2025-04-01T10:00:00.000Z'],
  ]);
  assert.match(result.details.join('\n'), /Arc Profiles \(“Work profile”\)/);
  assert.match(result.details.join('\n'), /Arc Notes/);

  // Again, merging: nothing is repeated.
  const again = await applyBrowserImport(arcData(), { mode: 'merge', isolate: [] }, store, noHistory);
  assert.deepEqual({ spaces: again.spaces, merged: again.merged, bookmarks: again.bookmarks, archived: again.archived }, { spaces: 0, merged: 2, bookmarks: 0, archived: 0 });
  // Again, adding: two more Spaces with everything but the favorites.
  const appended = await applyBrowserImport(arcData(), { mode: 'append', isolate: [] }, store, noHistory);
  assert.deepEqual({ spaces: appended.spaces, bookmarks: appended.bookmarks }, { spaces: 2, bookmarks: 14 });
  assert.equal(store.state.spaces.length, 4);
});

test('bookmarks from other browsers merge into the Space named after the browser', async () => {
  const store = new Store({ load: () => undefined, save: () => {} });
  const data = { name: 'Chrome', bookmarks: [{ kind: 'tab', url: 'https://example.com/', title: 'Example' }], bookmarkCount: 1, history: [], warnings: [] };
  assert.deepEqual(previewBrowserImport(data, store.state, false).skipped, []);
  assert.equal((await applyBrowserImport(data, { mode: 'merge', isolate: [] }, store, noHistory)).spaces, 1);
  const again = await applyBrowserImport(data, { mode: 'merge', isolate: [] }, store, noHistory);
  assert.deepEqual({ spaces: again.spaces, merged: again.merged, bookmarks: again.bookmarks, details: again.details }, { spaces: 0, merged: 1, bookmarks: 0, details: [] });
});
