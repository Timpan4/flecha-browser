// Control-Tab's order: the tabs you used, most recent first, then the ones you haven't seen yet.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { Store } = await loadModule('chromium/shared/store.ts');
const { decodeState } = await loadModule('chromium/shared/savedState.ts');

function storeWithTabs() {
  let json;
  const store = new Store({ load: () => undefined, save: (j) => { json = j; } });
  const tabs = {};
  for (const name of ['a', 'b', 'c']) tabs[name] = store.addTab(`https://example.com/${name}`).id;
  // Opened from c's page in the background, as Control-click or a middle click does.
  tabs.x = store.addTab('https://example.com/x', { activate: false, afterTabId: tabs.c }).id;
  // Used in this order, a clock tick apart.
  for (const [i, name] of ['a', 'b', 'c'].entries()) store.state.tabs[tabs[name]].lastActive = 1000 + i;
  return { store, tabs, saved: () => (store.saveNow(), json) };
}

test('a tab opened in the background comes after every tab you have used', () => {
  const { store, tabs } = storeWithTabs();
  assert.deepEqual(store.byRecency([tabs.a, tabs.b, tabs.c, tabs.x]), [tabs.c, tabs.b, tabs.a, tabs.x]);
});

test('showing a background tab makes it the most recent', () => {
  const { store, tabs } = storeWithTabs();
  store.activate(tabs.x);
  assert.equal(store.state.tabs[tabs.x].unseen, undefined);
  assert.deepEqual(store.byRecency([tabs.a, tabs.b, tabs.c, tabs.x]), [tabs.x, tabs.c, tabs.b, tabs.a]);
});

test('passing through a background tab without stopping on it leaves it unseen', () => {
  const { store, tabs } = storeWithTabs();
  store.activate(tabs.x, { recent: false });
  assert.deepEqual(store.byRecency([tabs.a, tabs.b, tabs.c, tabs.x]), [tabs.c, tabs.b, tabs.a, tabs.x]);
});

test('an unseen tab stays unseen after a restart', () => {
  const { tabs, saved } = storeWithTabs();
  const restored = new Store({ load: () => decodeState(saved()), save: () => {} });
  assert.deepEqual(restored.byRecency([tabs.a, tabs.b, tabs.c, tabs.x]), [tabs.c, tabs.b, tabs.a, tabs.x]);
});
