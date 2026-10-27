// The shell's tab sleep against a stand-in browser: a tab ages from when it left the screen, by
// active use that a restart doesn't lose; the rules run on the 300 s schedule only; Flecha's tabs
// are marked as its own to discard, without touching autoDiscardable.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const minute = 60_000;
const { createTabSleep } = await loadModule('chromium/shell/tabSleep.ts');

const area = (items = {}) => ({
  items,
  async get(keys) { return Object.fromEntries([keys].flat().filter((k) => k in items).map((k) => [k, structuredClone(items[k])])); },
  async set(patch) { Object.assign(items, structuredClone(patch)); },
});

/** A browser with Flecha tabs A and B (Chromium tabs 1 and 2), A on screen. */
function setup({ urls = {}, placement = {} } = {}) {
  const b = {
    clock: 0,
    idle: 'active',
    frozen: new Map(),
    managed: new Set(),
    freezeCalls: [],
    local: area(),
    session: area(),
    chromeTabs: [1, 2].map((id) => ({ id, autoDiscardable: true, discarded: false, audible: false, url: urls[id] ?? `https://example.com/${id}` })),
  };
  globalThis.chrome = {
    flechaPrivate: {
      getLiveTicks: async () => b.clock,
      setTabFreezing: async (id, freeze) => {
        b.freezeCalls.push([id, freeze]);
        if (freeze) b.frozen.set(id, true); else b.frozen.delete(id);
      },
      discardTab: async () => false,
      setTabSleepManaged: async (id, managed) => void (managed ? b.managed.add(id) : b.managed.delete(id)),
    },
    idle: { queryState: async () => b.idle },
    storage: { get local() { return b.local; }, get session() { return b.session; } },
    tabs: {
      query: async () => structuredClone(b.chromeTabs),
      update: async (id, fields) => Object.assign(b.chromeTabs.find((t) => t.id === id), fields),
    },
  };
  const store = {
    state: { tabs: { A: { lastActive: Date.now() }, B: { lastActive: Date.now() } }, windows: [{ id: 'w' }] },
    activeSpaceIn: () => ({ id: 's' }),
    locate: (id) => ({ space: { id: 's' }, zone: placement[id] ?? 'today', windowId: 'w' }),
  };
  b.live = new Map([['A', 1], ['B', 2]]);
  b.shown = new Set(['A']);
  b.start = () => (b.sleep = createTabSleep(store, b.live, () => b.shown));
  b.start();
  /** The browser quits (or crashes) and starts again: its session storage and marks are gone. */
  b.restart = () => {
    b.session = area();
    b.managed.clear();
    b.frozen.clear();
    b.start();
  };
  return b;
}

test('a tab starts aging when it leaves the screen, not at the sample before', async () => {
  const b = setup();
  await b.sleep.evaluate();
  b.clock += 5 * minute;
  b.shown = new Set(['B']);
  await b.sleep.evaluate();
  b.clock += 25 * minute;
  await b.sleep.evaluate();
  assert.equal(b.frozen.has(1), false, 'A has been in the background for 25 minutes of use');
  b.clock += 5 * minute;
  await b.sleep.evaluate();
  assert.equal(b.frozen.has(1), true, 'A, 30 minutes of use in the background, is frozen');
});

test('Flecha marks its tabs as its own to discard and leaves autoDiscardable to others', async () => {
  const b = setup();
  await b.sleep.evaluate();
  assert.deepEqual([...b.managed].sort(), [1, 2]);
  assert.deepEqual(b.chromeTabs.map((t) => t.autoDiscardable), [true, true], 'the recent ten are kept by the rules, not by autoDiscardable');
  // Into a Flechinha, say: no longer one of Flecha's tabs, so back to Chromium's defaults.
  b.shown = new Set(['B']);
  await b.sleep.evaluate();
  b.clock += 30 * minute;
  await b.sleep.evaluate();
  assert.equal(b.frozen.has(1), true);
  b.live.delete('A');
  await b.sleep.evaluate();
  assert.deepEqual([...b.managed], [2]);
  assert.equal(b.frozen.has(1), false);
});

test('a restart keeps the use up to the last heartbeat', async () => {
  const b = setup();
  await b.sleep.evaluate();
  b.shown = new Set(['B']);
  b.clock += minute;
  await b.sleep.evaluate();
  // 20 minutes of use, the last 4 of them after the last evaluation and up to a change of the
  // tabs on screen (a heartbeat); then the browser quits.
  b.clock += 16 * minute;
  await b.sleep.evaluate();
  b.clock += 4 * minute;
  await b.sleep.shownChanged();
  b.restart();
  await b.sleep.evaluate();
  b.clock += 10 * minute;
  await b.sleep.evaluate();
  assert.equal(b.frozen.has(1), true, 'A has had 30 minutes of use in the background');
});

test('hours with the browser closed don\'t age its tabs', async () => {
  const b = setup();
  await b.sleep.evaluate();
  b.shown = new Set(['B']);
  b.clock += minute;
  await b.sleep.evaluate();
  b.clock += 10 * minute;
  await b.sleep.evaluate();
  // Closed for 8 hours with the computer awake: the clock keeps running across the restart.
  b.clock += 8 * 60 * minute;
  b.restart();
  await b.sleep.evaluate();
  b.clock += 19 * minute;
  await b.sleep.evaluate();
  assert.equal(b.frozen.has(1), false, 'A has had 29 minutes of use in the background');
  b.clock += minute;
  await b.sleep.evaluate();
  assert.equal(b.frozen.has(1), true, 'A has had 30 minutes of use in the background');
});

test('a clock that went back (a reboot starts it again) adds nothing', async () => {
  const c = setup();
  c.clock = 100 * minute;
  await c.sleep.evaluate();
  c.shown = new Set(['B']);
  c.clock += minute;
  await c.sleep.evaluate();
  c.clock = 0;
  c.restart();
  await c.sleep.evaluate();
  c.clock += 29 * minute;
  await c.sleep.evaluate();
  assert.equal(c.frozen.has(1), false);
  c.clock += minute;
  await c.sleep.evaluate();
  assert.equal(c.frozen.has(1), true);
});

test('the rules run on the schedule only; a tab that comes on screen is woken at once', async () => {
  const b = setup();
  await b.sleep.evaluate();
  b.shown = new Set(['B']);
  b.clock += minute;
  await b.sleep.evaluate();
  b.clock += 40 * minute;
  // On screen changes between evaluations don't run the rules.
  await b.sleep.shownChanged();
  assert.equal(b.frozen.has(1), false);
  await b.sleep.evaluate();
  assert.equal(b.frozen.has(1), true);
  // Back on screen: unfrozen now, without waiting for the next evaluation.
  b.shown = new Set(['A']);
  await b.sleep.shownChanged();
  assert.equal(b.frozen.has(1), false);
  // On screen only between two evaluations still counts as used.
  b.clock += minute;
  b.shown = new Set(['B']);
  await b.sleep.shownChanged();
  b.clock += 29 * minute;
  await b.sleep.evaluate();
  assert.equal(b.frozen.has(1), false, 'A left the screen 29 minutes of use ago');
});

test('a page on Dia\'s tabSuspensionBlocklist (Notion) never sleeps', async () => {
  const b = setup({ urls: { 1: 'https://www.notion.so/team/page', 2: 'https://app.notion.com/p/1' } });
  b.shown = new Set();
  await b.sleep.evaluate();
  for (let i = 0; i < 60; i++) {
    b.clock += 5 * minute;
    await b.sleep.evaluate();
  }
  assert.deepEqual(b.freezeCalls, [], 'neither is frozen or discarded after 5 hours of use');
});

test('pinned tabs and Favorites sleep like any tab, as in Dia', async () => {
  const b = setup({ placement: { A: 'pinned', B: 'favorites' } });
  b.shown = new Set();
  await b.sleep.evaluate();
  b.clock += 30 * minute;
  await b.sleep.evaluate();
  assert.deepEqual([...b.frozen.keys()].sort(), [1, 2]);
});
