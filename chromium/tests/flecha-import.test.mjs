// Importing Flecha (Electron)'s state replaces yours only once you confirm, and keeps yours to restore.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { chooseFlechaState } = await loadModule('chromium/shell/lib/flechaImport.ts');
const { setAsideBeforeImport, stateBeforeImport } = await loadModule('chromium/shared/savedState.ts');
const { Store } = await loadModule('chromium/shared/store.ts');

function area(items = {}) {
  return {
    items,
    async get(keys) { return Object.fromEntries(keys.filter((k) => k in items).map((k) => [k, items[k]])); },
    async set(patch) { Object.assign(items, patch); },
  };
}

const sources = (native, file, answer) => {
  const asked = [];
  return {
    asked,
    readNative: async () => native,
    pickFile: async () => file,
    confirm: (message) => (asked.push(message), answer),
  };
};

test('a state file picked by hand is imported only once you confirm', async () => {
  const declined = sources(undefined, '{"file":1}', false);
  assert.equal(await chooseFlechaState(declined), undefined);
  assert.equal(declined.asked.length, 1);
  const accepted = sources(undefined, '{"file":1}', true);
  assert.equal(await chooseFlechaState(accepted), '{"file":1}');
});

test('the state the native host reads is imported only once you confirm, without a file picker', async () => {
  const from = sources('{"native":1}', '{"file":1}', true);
  from.pickFile = async () => assert.fail('the file picker opened');
  assert.equal(await chooseFlechaState(from), '{"native":1}');
  assert.equal(from.asked.length, 1);
  assert.equal(await chooseFlechaState(sources('{"native":1}', undefined, false)), undefined);
});

test('nothing chosen asks nothing', async () => {
  const from = sources(undefined, undefined, true);
  assert.equal(await chooseFlechaState(from), undefined);
  assert.deepEqual(from.asked, []);
});

test('the state an import replaces is kept and reads back as it was', async () => {
  const storage = area();
  assert.equal(await stateBeforeImport(storage), undefined);
  const store = new Store({ load: () => undefined, save: () => {} });
  store.state.spaces[0].name = 'Work';
  const tab = store.addTab('https://example.com/kept');
  await setAsideBeforeImport(storage, store.serialize());
  const before = await stateBeforeImport(storage);
  assert.equal(before.spaces[0].name, 'Work');
  assert.equal(before.tabs[tab.id].url, 'https://example.com/kept');
});
