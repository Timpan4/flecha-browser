// An incognito window applies your boosts but never writes one: a boost names the site it is for.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const writes = [];
const saved = { boosts: { 'kept.example.css': 'body {}' }, 'boosts:compiled': {} };
const event = { addListener: () => {} };
globalThis.chrome = {
  extension: { inIncognitoContext: true },
  storage: { local: { get: async () => structuredClone(saved), set: async (items) => void writes.push(items) } },
  tabs: { query: async () => [], onUpdated: event, onRemoved: event },
  webNavigation: { onCommitted: event, onDOMContentLoaded: event },
  debugger: { onEvent: event, onDetach: event },
  runtime: { getURL: (path) => `chrome-extension://flecha/${path}` },
};
const boosts = await loadModule('chromium/shell/features/boosts.ts');

test('an incognito window creates, edits and deletes no boost', async () => {
  await boosts.ready();
  assert.equal(await boosts.fileFor('https://private.example/page', 'script'), undefined);
  assert.equal(await boosts.create('private.example', 'styles'), undefined);
  assert.equal(await boosts.save('kept.example.css', 'body { color: red }'), undefined);
  await boosts.remove('kept.example.css');
  assert.deepEqual(writes, []);
});

test('an incognito window still finds your existing boosts', async () => {
  assert.equal(await boosts.fileFor('https://kept.example/', 'styles'), 'kept.example.css');
  assert.deepEqual(boosts.boostedHosts(), ['kept.example']);
});
