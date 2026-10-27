// Dragging a Space's button in the sidebar puts the Space where it was dropped.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { spaceDropBefore } = await loadModule('chromium/shell/lib/spaceOrder.ts');
const { Store } = await loadModule('chromium/shared/store.ts');

const order = ['a', 'b', 'c', 'd'];

test('a Space dropped on the near half of a button goes before that Space, on the far half after it', () => {
  assert.equal(spaceDropBefore(order, 'd', 'b', false), 'b');
  assert.equal(spaceDropBefore(order, 'a', 'c', true), 'd');
  assert.equal(spaceDropBefore(order, 'a', 'd', true), undefined);
});

test('a drop that leaves the order as it is moves nothing', () => {
  assert.equal(spaceDropBefore(order, 'b', 'b', false), null);
  assert.equal(spaceDropBefore(order, 'b', 'c', false), null);
  assert.equal(spaceDropBefore(order, 'b', 'a', true), null);
  assert.equal(spaceDropBefore(order, 'd', 'd', true), null);
  assert.equal(spaceDropBefore(order, 'x', 'a', false), null);
});

test('the Store takes the drop as the sidebar sends it', () => {
  const store = new Store({ load: () => undefined, save: () => {} });
  for (let i = 0; i < 3; i++) store.newSpace(false);
  const ids = store.state.spaces.map((s) => s.id);
  const drop = (dragged, over, after) => {
    const before = spaceDropBefore(store.state.spaces.map((s) => s.id), dragged, over, after);
    if (before !== null) store.moveSpace(dragged, before);
  };
  drop(ids[0], ids[3], true);
  assert.deepEqual(store.state.spaces.map((s) => s.id), [ids[1], ids[2], ids[3], ids[0]]);
  drop(ids[0], ids[1], false);
  assert.deepEqual(store.state.spaces.map((s) => s.id), ids);
});
