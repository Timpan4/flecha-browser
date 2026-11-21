import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { buildAdblock, LISTS } from '../shell/build-adblock.mjs';

test('reverse download completion preserves EasyList priority, rule IDs and the shared budget', async t => {
  const out = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-rules-'));
  t.after(() => fs.rm(out, { recursive: true, force: true }));
  const cache = path.join(out, 'cache');
  const replies = new Map();
  const build = buildAdblock(out, { cache, download: url => new Promise(resolve => replies.set(url, resolve)) });
  assert.deepEqual([...replies.keys()], Object.values(LISTS));
  replies.get(LISTS.easyprivacy)(new Response('[Adblock Plus 2.0]\n/track-one\n@@/allow\n/track-two'));
  // Leave room for one EasyPrivacy rule under Chromium's 30,000-rule guarantee.
  replies.get(LISTS.easylist)(new Response('[Adblock Plus 2.0]\n' +
    Array.from({ length: 29999 }, (_, i) => '/advert-' + i).join('\n')));
  await build;
  const rules = async name => JSON.parse(await fs.readFile(path.join(out, 'rules', name + '.json'), 'utf8'));
  const list = await rules('easylist');
  assert.equal(list.length, 29999);
  assert.equal(list.at(-1).id, list.length);
  assert.deepEqual(await rules('easyprivacy'),
    [{ priority: 2, action: { type: 'allow' }, condition: { urlFilter: '/allow' }, id: 1 }]);
});

test('failed refresh requires a cached list with supported network rules', async t => {
  const out = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-rules-cache-'));
  t.after(() => fs.rm(out, { recursive: true, force: true }));
  const cache = path.join(out, 'cache');
  const offline = () => { throw new Error('offline'); };
  await assert.rejects(buildAdblock(out, { cache, download: offline }), /no cache exists/);
  await buildAdblock(out, { cache, download: () => new Response('[Adblock Plus 2.0]\n||ads.example^') });
  await buildAdblock(out, { cache, refresh: true, download: offline });
  assert.equal(JSON.parse(await fs.readFile(path.join(out, 'rules/easylist.json'))).length, 1);
  await fs.writeFile(path.join(cache, 'easyprivacy.txt'), '[Adblock Plus 2.0]\n! truncated');
  await assert.rejects(buildAdblock(out, { cache, download: offline }), /no supported network rules/);
});
