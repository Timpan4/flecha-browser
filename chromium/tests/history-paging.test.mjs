// The History page reads every page of history, each once, newest first.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { PAGE, readHistory } = await loadModule('chromium/shell/lib/historyPaging.ts');

/**
 * chrome.history.search as Chromium answers it (HistoryBackend::QueryHistoryBasic): the visits in
 * [startTime, endTime), newest first, one result per page, at most maxResults; each result
 * reports the page's last visit of all, not the visit that matched.
 */
function fakeHistory(visits) {
  const last = new Map();
  for (const v of visits) last.set(v.url, Math.max(last.get(v.url) ?? 0, v.time));
  const search = async ({ text, startTime = 0, endTime = Infinity, maxResults = PAGE }) => {
    const seen = new Set();
    const found = [];
    for (const v of [...visits].sort((a, b) => b.time - a.time)) {
      if (v.time < startTime || v.time >= endTime || !v.url.includes(text) || seen.has(v.url)) continue;
      seen.add(v.url);
      if (found.length === maxResults) break;
      found.push({ id: v.url, url: v.url, title: v.url, lastVisitTime: last.get(v.url) });
    }
    return found;
  };
  return { search, urls: [...last.keys()] };
}

async function readAll(search, text = '') {
  let read = { entries: [], exhausted: false, asked: 0 };
  let reads = 0;
  while (!read.exhausted) {
    read = await readHistory(search, text, read.asked);
    if (++reads > 100) assert.fail('history never ran out');
  }
  return { entries: read.entries, reads };
}

test('pages visited again lately don\'t hide older pages from the next page', async () => {
  // 200 pages visited today and also long ago, and one older page visited once. A cursor at the
  // oldest page shown brings the 200 back from their old visits and stops before `old`.
  const visits = [{ url: 'https://example.com/old', time: 1 }];
  for (let i = 0; i < 200; i++) visits.push({ url: `https://example.com/${i}`, time: 5000 + i }, { url: `https://example.com/${i}`, time: 10000 + i });
  const { search, urls } = fakeHistory(visits);
  const { entries, reads } = await readAll(search);
  assert.equal(reads, 3);
  assert.deepEqual(entries.map((e) => e.url).sort(), [...urls].sort());
  assert.equal(entries.at(-1).url, 'https://example.com/old');
  const times = entries.map((e) => e.lastVisitTime);
  assert.deepEqual(times, [...times].sort((a, b) => b - a));
});

test('each read shows a page more, and history that fits in a page ends at once', async () => {
  const visits = Array.from({ length: 250 }, (_, i) => ({ url: `https://example.com/${i}`, time: i + 1 }));
  const { search } = fakeHistory(visits);
  const first = await readHistory(search, '', 0);
  assert.equal(first.entries.length, PAGE);
  assert.equal(first.entries[0].url, 'https://example.com/249');
  assert.equal(first.exhausted, false);
  const second = await readHistory(search, '', first.asked);
  assert.deepEqual(second.entries.slice(0, PAGE), first.entries);
  assert.equal(second.entries.length, 2 * PAGE);
  const third = await readHistory(search, '', second.asked);
  assert.equal(third.entries.length, 250);
  assert.equal(third.exhausted, true);
  const searched = await readHistory(search, '/24', 0);
  assert.deepEqual(searched.entries.map((e) => e.url), ['https://example.com/249', 'https://example.com/248', 'https://example.com/247', 'https://example.com/246', 'https://example.com/245', 'https://example.com/244', 'https://example.com/243', 'https://example.com/242', 'https://example.com/241', 'https://example.com/240', 'https://example.com/24']);
  assert.equal(searched.exhausted, true);
});

test('a page that comes back twice (your visit and an agent\'s) is shown once', async () => {
  const twice = { id: 'a', url: 'https://example.com/a', lastVisitTime: 2 };
  const read = await readHistory(async () => [twice, { ...twice }, { id: 'b', url: 'https://example.com/b', lastVisitTime: 1 }], '', 0);
  assert.deepEqual(read.entries.map((e) => e.id), ['a', 'b']);
  assert.equal(read.exhausted, true);
});
