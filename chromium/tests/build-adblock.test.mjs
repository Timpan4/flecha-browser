import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { buildAdblock, LISTS } from '../shell/build-adblock.mjs';

function subscription(url, rules) {
  if (url === LISTS.easylist) return '[Adblock Plus 2.0]\n! Title: EasyList\n' + rules +
    '\n! *** easylist:easylist_adult/adult_allowlist_popup.txt ***\n';
  return '[Adblock Plus 1.1]\n! Title: EasyPrivacy\n! *** easylist:easyprivacy/easyprivacy_allowlist_international.txt ***\n' +
    rules + '\n';
}

test('reverse download completion preserves EasyList priority, rule IDs and the shared budget', async t => {
  const out = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-rules-'));
  t.after(() => fs.rm(out, { recursive: true, force: true }));
  const cache = path.join(out, 'cache');
  const replies = new Map();
  const build = buildAdblock(out, { cache, download: url => new Promise(resolve => replies.set(url, resolve)) });
  assert.deepEqual([...replies.keys()], Object.values(LISTS));
  replies.get(LISTS.easyprivacy)(new Response(subscription(LISTS.easyprivacy, '/track-one\n@@/allow\n/track-two')));
  // Leave room for one EasyPrivacy rule under Chromium's 30,000-rule guarantee.
  replies.get(LISTS.easylist)(new Response(subscription(LISTS.easylist,
    Array.from({ length: 29999 }, (_, i) => '/advert-' + i).join('\n'))));
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
  const list = url => subscription(url, url === LISTS.easylist ? '||ads.example^\n/banner' : '@@/allow');
  await buildAdblock(out, { cache, download: url => new Response(list(url)) });
  await buildAdblock(out, { cache, refresh: true, download: offline });
  const prefix = '[Adblock Plus 2.0]\n! Title: EasyList\n||ads.example^\n';
  for (const invalid of [
    () => new Response(prefix),
    () => new Response(list(LISTS.easylist), { headers: { 'Content-Length': String(list(LISTS.easylist).length + 1) } }),
    () => new Response(list(LISTS.easylist).replace('! Title:', '! Checksum: invalid\n! Title:')),
  ]) {
    await buildAdblock(out, { cache, refresh: true, download: url => url === LISTS.easylist ? invalid() : new Response(list(url)) });
    assert.equal(await fs.readFile(path.join(cache, 'easylist.txt'), 'utf8'), list(LISTS.easylist));
    assert.equal(JSON.parse(await fs.readFile(path.join(out, 'rules/easylist.json'))).length, 2);
  }
  await fs.writeFile(path.join(cache, 'easyprivacy.txt'), subscription(LISTS.easyprivacy, '! truncated'));
  const warnings = t.mock.method(console, 'warn', () => {});
  await assert.rejects(buildAdblock(out, { cache, download: offline }), error =>
    error instanceof AggregateError && error.message.includes('easyprivacy') && error.errors[0].message === 'offline' &&
    error.errors[1].message === 'incomplete final filter list section');
  assert.ok(!warnings.mock.calls.some(call => call.arguments[0].includes('using the cached copy')));
});

test('a valid ABP checksum covers normalized UTF-8 and Content-Length covers the original bytes', async t => {
  const out = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-signed-rules-'));
  t.after(() => fs.rm(out, { recursive: true, force: true }));
  const signed = '[Adblock Plus 2.0]\r\n! Checksum: z2RKQ5QVHnDHGokvvaKKdg\r\n! Title: EasyList\r\n\r\n||ads.example^\r\n';
  const cache = path.join(out, 'cache');
  await buildAdblock(out, { cache, download: (url, options) => {
    assert.equal(options.headers['Accept-Encoding'], 'identity');
    const text = url === LISTS.easylist ? signed : subscription(url, '! précision\n@@/allow');
    return new Response(text, { headers: { 'Content-Length': String(Buffer.byteLength(text)) } });
  } });
  assert.equal(await fs.readFile(path.join(cache, 'easylist.txt'), 'utf8'), signed);
});
