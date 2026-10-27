// Importing bookmarks replaces a fresh install's empty Space, but never a Space that holds something.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { Store } = await loadModule('chromium/shared/store.ts');

const newStore = () => new Store({ load: () => undefined, save: () => {} });
const bookmarks = [{ kind: 'tab', url: 'https://example.com/', title: 'Example' }];

test('a fresh install’s empty Space gives way to the imported one', () => {
  const store = newStore();
  const space = store.importBookmarks('Imported', bookmarks);
  assert.deepEqual(store.state.spaces.map((s) => s.id), [space.id]);
});

test('a Space is kept when a closed window holds its only tabs, so reopening that window brings them back', () => {
  const store = newStore();
  const [original] = store.state.spaces;
  const second = store.openWindow().id;
  const tab = store.addTab('https://example.org/', { windowId: second });
  store.closeWindow(second);
  store.importBookmarks('Imported', bookmarks);
  assert.ok(store.state.spaces.some((s) => s.id === original.id));
  const reopened = store.reopenWindow();
  assert.equal(store.activeTabIn(reopened.id)?.id, tab.id);
});

test('a Space is kept when favorites are its only tabs', () => {
  const store = newStore();
  const [original] = store.state.spaces;
  store.toggleFavorite(store.addTab('https://example.org/').id);
  const space = store.importBookmarks('Imported', bookmarks);
  assert.deepEqual(store.state.spaces.map((s) => s.id), [original.id, space.id]);
});
