// Search suggestions for the command bar: what is sent, how answers are read, and that only the latest query's answer shows.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { MAX_SUGGESTIONS, SUGGEST_INTERVAL_MS, fetchSuggestions, parseSuggestions, suggestable, suggestionRequests } = await loadModule('chromium/shell/lib/suggest.ts');
const { SEARCH_ENGINES, resolveInput, searchUrl } = await loadModule('chromium/shell/lib/url.ts');

test('the answer formats of every engine are read', () => {
  // Shapes as each endpoint answered on 2026-10-26.
  const google = '["githu",["github","https://github.com/","github copilot"],["","GitHub",""],[],{"google:suggestrelevance":[1250,1150,800],"google:suggesttype":["QUERY","NAVIGATION","QUERY"]}]';
  assert.deepEqual(parseSuggestions(google, 'githu'), ['github', 'github copilot']);
  const calculator = '["1+1",["= 2","1+1 math"],["Calculator",""],[],{"google:suggesttype":["CALCULATOR","QUERY"]}]';
  assert.deepEqual(parseSuggestions(calculator, '1+1'), ['1+1 math']);
  assert.deepEqual(parseSuggestions(")]}'\n" + '["a",["ab"]]', 'a'), ['ab']);
  const bing = '["weather",["weather","weather stockholm"],[],[],{"google:suggestrelevance":[1300,1299]}]';
  assert.deepEqual(parseSuggestions(bing, 'weather'), ['weather stockholm']);
  // DuckDuckGo, Brave, Ecosia and Kagi: the query and a list of strings.
  assert.deepEqual(parseSuggestions('["weather",["weather radar","Weather Radar","weather online"]]', 'weather'), ['weather radar', 'weather online']);
  assert.ok(Object.values(SEARCH_ENGINES).every((engine) => engine.suggest.startsWith('https://') && engine.suggest.includes('%s')));
});

test('answers for another query, and malformed ones, have no suggestions', () => {
  assert.deepEqual(parseSuggestions('["weathe",["weather"]]', 'weather'), []);
  assert.deepEqual(parseSuggestions('not json', 'a'), []);
  assert.deepEqual(parseSuggestions('{"a":1}', 'a'), []);
  assert.deepEqual(parseSuggestions('["a","b"]', 'a'), []);
  assert.deepEqual(parseSuggestions('["a",[1,null,{"q":"x"},"ab"],null,null,"meta"]', 'a'), ['ab']);
  const many = JSON.stringify(['q', Array.from({ length: 15 }, (_, i) => `q ${i}`)]);
  assert.equal(parseSuggestions(many, 'q').length, MAX_SUGGESTIONS);
});

test('addresses, internal pages and file paths are never sent', () => {
  for (const query of ['github.com', 'https://example.com/a?b', 'localhost:3000', 'chrome://settings', 'about:blank', 'file:///etc/passwd', 'javascript:alert(1)', 'mailto:a@b.c', '/home/me/notes.txt', '~/notes', './x', '\\\\server\\share', 'C:\\Users\\me', '  ']) {
    assert.equal(suggestable(query), false, query);
  }
  for (const query of ['weather', 'how to: bake bread', 'c++ vector', '1+1', 'what is 3.5 / 2']) assert.equal(suggestable(query), true, query);
});

test('a picked suggestion is searched for, even when it reads as an address', () => {
  assert.equal(searchUrl('github.com', 'duckduckgo'), 'https://duckduckgo.com/?q=github.com');
  assert.equal(resolveInput('github.com', 'duckduckgo'), 'https://github.com');
});

test('the request goes to the engine without cookies, and the answer is decoded by its charset', async () => {
  const requests = [];
  const latin1 = Uint8Array.from([...'["caf\u00e9",["caf\u00e9 bar"]]'].map((c) => c.charCodeAt(0)));
  const load = async (url, init) => {
    requests.push({ url, credentials: init.credentials });
    return new Response(latin1, { headers: { 'Content-Type': 'text/javascript; charset=ISO-8859-1' } });
  };
  assert.deepEqual(await fetchSuggestions('google', 'café', new AbortController().signal, load), ['café bar']);
  assert.deepEqual(requests, [{ url: 'https://www.google.com/complete/search?client=chrome-omni&q=caf%C3%A9', credentials: 'omit' }]);
  const failed = async () => new Response('["café",["café bar"]]', { status: 503 });
  assert.deepEqual(await fetchSuggestions('google', 'café', new AbortController().signal, failed), []);
});

/** A clock the test moves by hand. */
function manualClock() {
  let now = 0;
  const timers = new Set();
  return {
    now: () => now,
    setTimeout: (run, ms) => {
      const timer = { at: now + ms, run };
      timers.add(timer);
      return timer;
    },
    clearTimeout: (timer) => timers.delete(timer),
    advance(ms) {
      now += ms;
      for (const timer of [...timers]) if (timer.at <= now) timers.delete(timer), timer.run();
    },
  };
}

/** A load whose answers the test gives, one per request; `abortable: false` answers even after an abort. */
function pendingLoads({ abortable = true } = {}) {
  const calls = [];
  const load = (query, signal) =>
    new Promise((resolve, reject) => {
      calls.push({ query, signal, resolve, reject });
      if (abortable) signal.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')));
    });
  return { calls, load };
}

const settle = () => new Promise((resolve) => setImmediate(resolve));

for (const abortable of [true, false]) {
  test(`an answer for an older query never shows, and its request is aborted (load ${abortable ? 'stops' : 'ignores the abort'})`, async () => {
    const clock = manualClock();
    const { calls, load } = pendingLoads({ abortable });
    const shown = [];
    const request = suggestionRequests(load, (query, list) => shown.push([query, list]), clock);
    request('wea');
    assert.equal(calls.length, 1);
    clock.advance(SUGGEST_INTERVAL_MS);
    request('weat');
    assert.equal(calls[0].signal.aborted, true);
    assert.equal(calls.length, 2);
    // The newer answer comes first, then the older one: only the newer shows.
    calls[1].resolve(['weather']);
    await settle();
    calls[0].resolve(['wealth']);
    await settle();
    assert.deepEqual(shown, [['weat', ['weather']]]);
  });
}

test('requests go at most every interval, and only for the latest query', async () => {
  const clock = manualClock();
  const { calls, load } = pendingLoads();
  const shown = [];
  const request = suggestionRequests(load, (query, list) => shown.push([query, list]), clock);
  request('a');
  clock.advance(10);
  request('ab');
  clock.advance(10);
  request('abc');
  assert.deepEqual(calls.map((c) => c.query), ['a']);
  clock.advance(SUGGEST_INTERVAL_MS - 21);
  assert.deepEqual(calls.map((c) => c.query), ['a']);
  clock.advance(1);
  assert.deepEqual(calls.map((c) => c.query), ['a', 'abc']);
  calls[1].resolve(['abcd']);
  await settle();
  assert.deepEqual(shown, [['abc', ['abcd']]]);
});

test('stopping drops the pending request and any answer; failures show nothing', async () => {
  const clock = manualClock();
  const { calls, load } = pendingLoads();
  const shown = [];
  const request = suggestionRequests(load, (query, list) => shown.push([query, list]), clock);
  request('a');
  request(undefined);
  calls[0].resolve(['ab']);
  clock.advance(SUGGEST_INTERVAL_MS);
  request('b');
  calls[1].reject(new TypeError('Failed to fetch'));
  await settle();
  assert.equal(calls.length, 2);
  assert.deepEqual(shown, []);
});
