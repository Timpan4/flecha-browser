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
  // Logins, ports, queries, fragments and paths after a host, an IP address, a bare scheme (Sol check SUGGEST).
  for (const query of ['example.com?token=secret', 'example.com#section', 'example.com:8443', 'me@example.com', 'user:password@example.com', '192.168.1.10/private', '192.168.1.10', '10.0.0.1:8080', '[::1]:3000', 'intranet.example.local/wiki', 'bücher.de/konto', 'chrome:', 'view-source:example.com']) {
    assert.equal(suggestable(query), false, query);
  }
  // Other spellings of an address, parsed as the browser would (Sol check SUGGEST2).
  for (const query of ['example.com.?token=secret', '127.1/private?token=secret', '0x7f.1', 'example。com?token=secret', 'intranet/wiki', 'intranet:8080', 'example.com?token=secret more words', 'me@example.com hello', 'mailto:someone']) {
    assert.equal(suggestable(query), false, query);
  }
  // Parts the URL parser drops (a dot segment, a default port), and address parts in any word of a
  // longer query, pasted lines included (Sol check SUGGEST3).
  for (const query of ['example.com/private/.. hello', 'example.com:80 hello', 'example.com:080 hello', 'example.com/%2e%2e hello', '[::1]:80 hello', 'localhost:80 hello', 'intranet/private/..', 'intranet/%2e%2e', 'intranet\\private\\..', 'intranet/', 'example.com\n/private?token=secret', 'example.com\t/private', 'notes user:secret@host hello', 'user:secret@host hello', 'https:example.com/private?token=secret words', 'see example.com?token=secret', 'site:example.com cats', 'chrome:']) {
    assert.equal(suggestable(query), false, JSON.stringify(query));
  }
  // Percent-escaped hosts, and ports or paths after a numeric host in a longer query (Sol check SUGGEST4).
  for (const query of ['%65xample%2ecom', '%65xample%2ecom/private', 'notes 127.0.0.1:8080', 'notes 127.1:80', 'notes 127.0.0.1/80', 'notes %65xample%2ecom%3Fq', 'see 10.0.0.1:22 now']) {
    assert.equal(suggestable(query), false, JSON.stringify(query));
  }
  // Four-part IPv4 addresses, ports or paths after a numeric IPv4 address, a trailing `/` (or `?`, `#` after a host)
  // and bare schemes the browser opens, in any word (Sol check SUGGEST5).
  for (const query of ['notes 127.0.0.1', 'notes 127。0。0。1', 'notes 0x7f.0.0.1', '2130706433:80', 'notes 2130706433:80', 'notes 2130706433/80', 'notes 017700000001:80', 'notes intranet/', 'notes example.com/', 'notes example.com?', 'notes example.com#', 'notes chrome:', 'chrome: notes', 'notes file:', 'notes about:']) {
    assert.equal(suggestable(query), false, JSON.stringify(query));
  }
  // Sentence punctuation around an address, `?` or `#` after a number the browser opens as an IP address,
  // and every scheme Chromium handles itself (Sol check SUGGEST6).
  for (const query of ['notes 127.0.0.1.', 'see (127.0.0.1).', 'notes 127.0.0.1,', 'notes 2130706433?', '2130706433#', 'notes 0x7f000001#', 'notes chrome-search:', 'notes isolated-app:', 'notes chrome-distiller:', '"chrome:" notes', 'see example.com/.', 'notes [::1].']) {
    assert.equal(suggestable(query), false, JSON.stringify(query));
  }
  // A four-part address ending in a Unicode full stop, which the URL parser reads as a dot (Sol check SUGGEST7).
  for (const query of ['notes 127。0。0。1。', 'notes 127.0.0.1。', 'notes 127｡0｡0｡1｡', 'notes 0x7f。0。0。1。', 'notes 0177。0。0。1。', 'notes 127.0.0.1.。']) {
    assert.equal(suggestable(query), false, JSON.stringify(query));
  }
  // CJK punctuation, other non-ASCII symbols, and text with no spaces around the address (Sol check SUGGEST8).
  for (const query of ['notes 127。0。0。1。、', 'notes 127。0。0。1。」', 'notes 127.0.0.1.』', 'notes 「127。0。0。1。」', 'サーバー「127.0.0.1」です', 'notes 🙂127.0.0.1🙂', 'notes «127.0.0.1»', '「example.com」']) {
    assert.equal(suggestable(query), false, JSON.stringify(query));
  }
  // Invisible characters the URL parser drops (Sol check SUGGEST9), and a lone host with only symbols beside it.
  for (const query of ['notes (127.\u{200b}0.0.1)', 'example.\u{200b}com', '127.\u{200b}1', 'example\u{ad}.com', '❤ example.com', '🙂 example.com', '👍🏽 example.com']) {
    assert.equal(suggestable(query), false, JSON.stringify(query));
  }
  // A word of punctuation alone still counts as a word, as before (Sol check SUGGEST10).
  for (const query of ['「こんにちは」 世界', 'サーバー「example.com」は遅い', 'notes «quoted» 🙂', '-- done.', '! e.g.', '... U.S.A.', '❤\u{fe0f} done.']) {
    assert.equal(suggestable(query), true, JSON.stringify(query));
  }
  for (const query of ['notes 1984?', 'what is 42?', 'e.g. this', 'the U.S.A. today', 'version 1.2.3.', 'notes 今日は晴れ。', 'all done.','"quoted words"', '(parenthetical note)']) assert.equal(suggestable(query), true, query);
  // Two or three parts are a query among other words, as in Chromium; so are dates, times and "re:".
  for (const query of ['notes 127.1', 'version 1.2.3', '2026/10/26 events', '08:30 standup', 're: notes', 'is it down?', 'ratio 4:3']) assert.equal(suggestable(query), true, query);
  // A lone number is a query, as in Chromium, though it also spells an IPv4 address; dates and times are too.
  for (const query of ['1984', '2130706433', '0x80070005', '0306406152', 'meeting 1/2/2026', 'at 10:30:15', '100%', '50%25 off']) assert.equal(suggestable(query), true, query);
  for (const query of ['weather', 'how to: bake bread', 're: meeting notes', 'c++ vector', 'c++', '1+1', 'what is 3.5 / 2', 'version 1.2.3', 'what is example.com', 'example.com is down', 'weather?', 'C#', 'C# generics', '1/2 cup', '10:30 meeting', '16:9 monitor', '@handle posts', '#hashtag']) assert.equal(suggestable(query), true, query);
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

test('stopping drops the pending request and any answer; a failure shows no suggestions for its query', async () => {
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
  // Empty, so suggestions shown earlier for "b" don't stay (Sol check SUGGEST4).
  assert.deepEqual(shown, [['b', []]]);
});
