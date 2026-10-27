// One undo takes back a whole Space deletion, bulk close or bulk move, and leaves later changes alone.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { Store } = await loadModule('chromium/shared/store.ts');

function twoWindows() {
  const store = new Store({ load: () => undefined, save: () => {} });
  const work = store.newSpace(false).id;
  const first = store.focusedWindowId;
  const second = store.openWindow().id;
  return { store, first, second, personal: store.state.spaces[0].id, work };
}

const urls = (store, ids) => ids.map((id) => store.state.tabs[id]?.url);
const today = (store, windowId, spaceId) => store.state.windows.find((w) => w.id === windowId).spaces[spaceId]?.today ?? [];

test('undoing a Space deletion brings back the Space, its folders and tabs, and where each window was', () => {
  const { store, first, second, personal, work } = twoWindows();
  const third = store.newSpace(false).id;
  store.switchSpace(work, first);
  store.switchSpace(work, second);
  const pinned = store.addTab('https://example.com/pinned', { windowId: first });
  store.togglePin(pinned.id, first);
  const folder = store.newFolder('Docs', { spaceId: work });
  const inFolder = store.createTab('https://example.com/doc', 'Doc', { zone: 'pinned', spaceId: work, folderId: folder });
  const a = store.addTab('https://example.com/a', { windowId: first });
  const b = store.addTab('https://example.com/b', { windowId: second });
  const pinnedBefore = structuredClone(store.state.spaces[1].pinned);

  store.deleteSpace(work);
  assert.equal(store.undoLabel, 'Delete “Space 2”');
  assert.equal(store.state.archive.length, 4);
  assert.equal(store.undo(), 'Delete “Space 2”');

  assert.deepEqual(store.state.spaces.map((s) => s.id), [personal, work, third]);
  assert.deepEqual(store.state.spaces[1].pinned, pinnedBefore);
  assert.deepEqual(urls(store, [pinned.id, inFolder.id]), ['https://example.com/pinned', 'https://example.com/doc']);
  assert.deepEqual(today(store, first, work), [a.id]);
  assert.deepEqual(today(store, second, work), [b.id]);
  assert.equal(store.activeSpaceIn(first).id, work);
  assert.equal(store.activeSpaceIn(second).id, work);
  assert.equal(store.activeTabIn(second).id, b.id);
  assert.deepEqual(store.state.archive, []);
  assert.equal(store.undoLabel, undefined);
});

test('one undo restores every tab of a bulk delete, in place, and keeps tabs opened since', () => {
  const { store, first, personal } = twoWindows();
  const tabs = ['a', 'b', 'c', 'd'].map((n) => store.addTab(`https://example.com/${n}`, { windowId: first }));
  const favorite = store.addTab('https://example.com/fav', { windowId: first });
  store.toggleFavorite(favorite.id, first);
  const order = today(store, first, personal).slice();

  store.removeTabs([tabs[0].id, tabs[2].id, favorite.id]);
  const later = store.addTab('https://example.com/later', { windowId: first });
  assert.equal(store.undoLabel, 'Delete 3 Tabs');
  store.undo();

  assert.deepEqual(today(store, first, personal).filter((id) => id !== later.id), order);
  assert.ok(today(store, first, personal).includes(later.id));
  assert.deepEqual(store.state.favorites, [favorite.id]);
  assert.equal(store.state.tabs[favorite.id].homeUrl, 'https://example.com/fav');
  assert.equal(store.undoLabel, undefined);
});

test('moves grouped as one change are undone together', () => {
  const { store, first, personal, work } = twoWindows();
  const tabs = ['a', 'b', 'c'].map((n) => store.addTab(`https://example.com/${n}`, { windowId: first }));
  const order = today(store, first, personal).slice();
  store.undoable('Move 2 Tabs', () => {
    store.placeTab(tabs[0].id, { zone: 'pinned', spaceId: work }, first);
    store.placeTab(tabs[2].id, { zone: 'pinned', spaceId: work }, first);
  });
  assert.equal(store.state.spaces[1].pinned.length, 2);
  assert.equal(store.undo(), 'Move 2 Tabs');
  assert.deepEqual(today(store, first, personal), order);
  assert.deepEqual(store.state.spaces[1].pinned, []);
  assert.equal(store.state.tabs[tabs[0].id].homeUrl, undefined);
  assert.equal(store.undo(), undefined);
});

test('a deletion inside a grouped change is part of the same undo', () => {
  const { store, first, work } = twoWindows();
  store.switchSpace(work, first);
  store.addTab('https://example.com/a', { windowId: first });
  store.undoable('Tidy Up', () => {
    store.deleteSpace(work);
    store.clearToday(first);
  });
  assert.equal(store.undoLabel, 'Tidy Up');
  store.undo();
  assert.equal(store.state.spaces.length, 2);
  assert.equal(store.undoLabel, undefined);
});

test('undoing a folder deletion puts the folder back with its tabs, whether they were kept or deleted', () => {
  for (const keepTabs of [true, false]) {
    const { store, work } = twoWindows();
    const before = store.createTab('https://example.com/before', 'Before', { zone: 'pinned', spaceId: work });
    const folder = store.newFolder('Docs', { spaceId: work });
    const inside = ['x', 'y'].map((n) => store.createTab(`https://example.com/${n}`, n, { zone: 'pinned', spaceId: work, folderId: folder }));
    const after = store.createTab('https://example.com/after', 'After', { zone: 'pinned', spaceId: work });
    const pinned = structuredClone(store.state.spaces[1].pinned);
    store.deleteFolder(folder, keepTabs);
    store.undo();
    assert.deepEqual(store.state.spaces[1].pinned, pinned, `keepTabs: ${keepTabs}`);
    assert.deepEqual(urls(store, [before.id, ...inside.map((t) => t.id), after.id]), ['before', 'x', 'y', 'after'].map((n) => `https://example.com/${n}`));
  }
});

test('Clear Today Tabs and Close Other Tabs undo in one step', () => {
  const { store, first, personal } = twoWindows();
  const tabs = ['a', 'b', 'c'].map((n) => store.addTab(`https://example.com/${n}`, { windowId: first }));
  const order = today(store, first, personal).slice();
  store.clearToday(first);
  assert.equal(store.undo(), 'Clear Today Tabs');
  assert.deepEqual(today(store, first, personal), order);
  store.closeTodayTabs(tabs[1].id, 'others');
  assert.equal(store.undo(), 'Close Other Tabs');
  assert.deepEqual(today(store, first, personal), order);
});

test('a change that changes nothing leaves nothing to undo', () => {
  const { store } = twoWindows();
  assert.deepEqual(store.removeTabs(['missing']), []);
  store.undoable('Nothing', () => {});
  assert.equal(store.undoLabel, undefined);
});

test('undo leaves a window reopened since closed no more', () => {
  const { store, first, second } = twoWindows();
  store.addTab('https://example.com/kept', { windowId: second });
  const gone = store.addTab('https://example.com/gone', { windowId: first });
  store.closeWindow(second);
  store.removeTabs([gone.id]);
  store.reopenWindow();
  store.undo();
  assert.deepEqual(store.state.closedWindows, []);
  assert.equal(store.state.windows.filter((w) => w.id === second).length, 1);
});

test('undoing a reorder moves back only the tab that was moved', () => {
  const { store, first, personal, work } = twoWindows();
  for (const n of ['a', 'b', 'c']) store.addTab(`https://example.com/${n}`, { windowId: first });
  const [top, middle, moved] = today(store, first, personal);
  store.undoable('Move Tab', () => store.moveTab(moved, { zone: 'today', beforeTabId: top }, first));
  store.moveToSpace(top, work);
  assert.equal(store.undo(), 'Move Tab');
  assert.deepEqual(today(store, first, work), [top]);
  assert.deepEqual(today(store, first, personal), [middle, moved]);
});

test('undoing a Space deletion brings back its split view', () => {
  const { store, first, work } = twoWindows();
  store.switchSpace(work, first);
  const left = store.addTab('https://example.com/left', { windowId: first });
  const right = store.addTab('https://example.com/right', { windowId: first, activate: false });
  store.splitWith(right.id, 'right', first);
  const before = structuredClone(store.state.windows.find((w) => w.id === first).spaces[work]);
  store.deleteSpace(work);
  store.undo();
  const ws = store.state.windows.find((w) => w.id === first).spaces[work];
  assert.deepEqual(ws.split, [left.id, right.id]);
  assert.deepEqual(ws, before);
});

test('a deleted tab whose window has closed since stays in the archive', () => {
  const { store, second } = twoWindows();
  const tab = store.addTab('https://example.com/only', { windowId: second });
  store.clearToday(second);
  store.closeWindow(second);
  assert.equal(store.undo(), 'Clear Today Tabs');
  assert.equal(store.state.tabs[tab.id], undefined);
  assert.deepEqual(store.state.archive.map((a) => a.url), ['https://example.com/only']);
});

test('undoing a move out of pinned gives the tab back its home', () => {
  const { store, first, personal } = twoWindows();
  const tab = store.addTab('https://home.example/', { windowId: first });
  store.togglePin(tab.id, first);
  store.updateTab(tab.id, { url: 'https://home.example/deep/page' });
  store.undoable('Move Tab', () => store.placeTab(tab.id, { zone: 'today', spaceId: personal }, first));
  assert.equal(store.state.tabs[tab.id].homeUrl, undefined);
  store.undo();
  assert.equal(store.state.tabs[tab.id].homeUrl, 'https://home.example/');
});

test('undo brings back archive entries the change pushed out', () => {
  const { store, first } = twoWindows();
  const old = Array.from({ length: 500 }, (_, i) => ({ url: `https://old.example/${i}`, title: '', closedAt: i }));
  store.state.archive = [...old];
  store.addTab('https://example.com/a', { windowId: first });
  store.addTab('https://example.com/b', { windowId: first });
  store.clearToday(first);
  assert.equal(store.state.archive.at(-1).url, 'https://old.example/497');
  store.undo();
  assert.deepEqual(store.state.archive, old);
});
