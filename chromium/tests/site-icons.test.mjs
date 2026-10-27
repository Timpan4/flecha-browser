// An incognito window's site icons stay in memory: no host it visited is written to disk.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { setImmediate } from 'node:timers/promises';
import { loadModule } from './load-module.mjs';

const writes = [];
globalThis.chrome = {
  extension: { inIncognitoContext: true },
  storage: { local: { get: async () => ({}), set: async (items) => void writes.push(items) } },
};
globalThis.fetch = async () => new Response(new Uint8Array([1, 2, 3]), { headers: { 'content-type': 'image/png' } });
const { SiteIcons } = await loadModule('chromium/shell/siteIcons.ts');

test('incognito site icons are kept in memory only', async (t) => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  let found;
  const icons = new SiteIcons((icon) => (found = icon));
  await icons.want(['private.example']);
  while (!found) await setImmediate();
  await setImmediate();
  t.mock.timers.tick(60_000);
  assert.ok((await icons.all())['private.example']);
  assert.deepEqual(writes, []);
});
