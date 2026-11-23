// Records the Chromium shell's Boosts behaviour for tests/parity.rs.
// Run from the repository root: node rewrite/crates/flecha-boosts/scripts/generate-boosts-fixture.ts
//
// The shell's own modules run here unchanged. features/boosts.ts runs against a mocked `chrome`
// that records what the shell asks Chromium to do: the user scripts it registers and executes,
// the styles it inserts, the tabs it reloads and what it writes to chrome.storage.local.
import { createHash } from 'node:crypto';
import { writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import vm from 'node:vm';
import { loadModule } from '../../../../chromium/tests/load-module.mjs';
import type { CompiledBoost } from '../../../../chromium/shell/features/boosts';
import type { BoostKind } from '../../../../chromium/shell/lib/boostFiles';

const OUT = 'rewrite/crates/flecha-boosts/tests/fixtures/boosts-parity.jsonl';

const files: typeof import('../../../../chromium/shell/lib/boostFiles') = await loadModule('chromium/shell/lib/boostFiles.ts');

const { compileBoost }: typeof import('../../../../chromium/shell/features/compileBoost') =
  await loadModule('chromium/shell/features/compileBoost.ts');

const { transform }: typeof import('sucrase') = createRequire(`${process.cwd()}/package.json`)('sucrase');

type Json = null | boolean | number | string | Json[] | { [key: string]: Json };

const rows: Json[] = [];

/** A JSON copy, as the shell's values look after a round trip through storage or IPC. */
function plain<T>(value: T): Json {
  return JSON.parse(JSON.stringify(value ?? null));
}

// ---- names, matching and the host a new Boost is named after -----------------

const names = [
  'github.com.ts', 'github.com.css', 'all.css', 'all.ts', 'a.ts.css', 'a.css.ts', '.ts', '.css', '..ts', '.hidden.css',
  'x.ts', 'x.TS', 'x.Css', 'ts', 'css', 'github.com.js', 'github.com.ts ', ' github.com.ts', 'Foo Bar.css',
  'a\tb.ts', 'a\nb.ts', 'a\rb.css', 'a\u{2028}b.ts', 'a\u{2029}b.css', 'a\u{85}b.ts', 'a"b\\c</script>.ts', 'bücher.example.css',
  '\u{1F600}.ts', '', '.', 'a.ts\n', 'www.github.com.css',
];

for (const name of names) rows.push({ kind: 'name', name, parsed: plain(files.parseBoostName(name)) });

const hosts = [
  'all', 'github.com', 'GitHub.com', 'gist.github.com', 'com', 'github.com.', 'bücher.example', 'xn--bcher-kva.example',
  '127.0.0.1', '[::1]', '::1', 'settings', 'boosts', 'server', 'localhost', '', '例え.テスト', 'xn--r8jz45g.xn--zckzah',
];

const urls = [
  'https://github.com/', 'http://github.com/x?y#z', 'https://GitHub.COM/', 'https://gist.github.com/a', 'https://a.b.gist.github.com/',
  'https://notgithub.com/', 'https://github.com.evil.example/', 'https://github.com./', 'https://github.com:8443/p',
  'https://user:pass@github.com/', 'https://www.github.com/', 'https://WWW.GitHub.com/', 'https://www.www.github.com/',
  'http://[::1]:8080/', 'http://127.0.0.1/', 'http://0x7f.1/', 'http://1.2.3/', 'https://bücher.example/',
  'https://xn--bcher-kva.example/', 'http://例え.テスト/', 'https://%67ithub.com/', 'https://ex%41mple.com/',
  'https://github.com\t/', ' https://github.com/', 'HTTPS://github.com/', 'https:github.com', 'https://localhost:3000/',
  'blob:https://github.com/0f0f', 'about:blank', 'about:srcdoc', 'data:text/html,hi', 'file:///etc/hosts',
  'file://server/share', 'chrome://settings/', 'flecha://boosts', 'chrome-extension://abcdefghijklmnop/boosts.html',
  'ws://github.com/', 'ftp://github.com/', 'view-source:https://github.com/', 'javascript:alert(1)', 'not a url', '',
  'https://', 'https://a..b/', 'httpx://github.com/', 'https://www./',
];

for (const url of urls) {
  rows.push({
    kind: 'url',
    url,
    host: plain(files.boostHostFor(url)),
    matches: hosts.filter((host) => files.boostMatches(url, host)),
  });
}

rows.push({ kind: 'url-hosts', hosts });

// ---- the mocked extension APIs ------------------------------------------------

type Tab = { id: number; url: string };

type Stored = { boosts?: Record<string, string>; boostsCompiled?: Record<string, CompiledBoost> };

type Navigation = { tabId: number; frameId: number; url: string; documentId?: string };

type Listener = (details: Navigation) => void;

type Registered = { id: string; code: string; matches: string[]; world: string; runAt: string; allFrames: boolean };

type Call =
  | { api: 'storage.set'; items: Json }
  | { api: 'unregister' }
  | { api: 'register'; scripts: Registered[] }
  | { api: 'execute'; tabId: number; documentId: string; code: string; world: string }
  | { api: 'insertCSS' | 'removeCSS'; tabId: number; css: string; origin: string }
  | { api: 'consoleError'; tabId: number; documentId: string; message: string; world: string }
  | { api: 'reload'; tabId: number };

type UserScript = { id: string; matches: string[]; js: { code: string }[]; world: string; runAt: string; allFrames?: boolean };

type Target = { tabId: number; documentIds: string[] };

let instance = 0;

async function shell(stored: Stored, tabs: Tab[]) {
  const calls: Call[] = [];
  const listeners = new Map<string, Listener>();
  const listen = (key: string) => ({ addListener: (fn: Listener) => void listeners.set(key, fn) });
  const ignore = { addListener: () => undefined };

  Object.assign(globalThis, {
    chrome: {
      extension: { inIncognitoContext: false },
      storage: {
        local: {
          get: async () => structuredClone(stored),
          set: async (items: Stored) => void calls.push({ api: 'storage.set', items: plain(items) }),
        },
      },
      userScripts: {
        getScripts: async () => [],
        unregister: async () => void calls.push({ api: 'unregister' }),
        register: async (scripts: UserScript[]) =>
          void calls.push({
            api: 'register',
            scripts: scripts.map((s) => ({ id: s.id, code: s.js[0].code, matches: s.matches, world: s.world, runAt: s.runAt, allFrames: s.allFrames ?? false })),
          }),
        execute: async ({ target, js, world }: { target: Target; js: { code: string }[]; world: string }) => {
          calls.push({ api: 'execute', tabId: target.tabId, documentId: target.documentIds[0], code: js[0].code, world });

          return [{}];
        },
      },
      scripting: {
        insertCSS: async ({ target, css, origin }: { target: Target; css: string; origin: string }) =>
          void calls.push({ api: 'insertCSS', tabId: target.tabId, css, origin }),
        removeCSS: async ({ target, css, origin }: { target: Target; css: string; origin: string }) =>
          void calls.push({ api: 'removeCSS', tabId: target.tabId, css, origin }),
        executeScript: async ({ target, args, world }: { target: Target; args: string[]; world: string }) => {
          calls.push({ api: 'consoleError', tabId: target.tabId, documentId: target.documentIds[0], message: args[0], world });

          return [{}];
        },
      },
      tabs: {
        query: async () => structuredClone(tabs),
        reload: async (tabId: number) => void calls.push({ api: 'reload', tabId }),
        onUpdated: ignore,
        onRemoved: ignore,
      },
      webNavigation: { onCommitted: listen('committed'), onDOMContentLoaded: listen('domReady') },
      debugger: { onEvent: ignore, onDetach: ignore },
      runtime: { getURL: (path: string) => `chrome-extension://flecha/${path}` },
    },
  });

  const boosts: typeof import('../../../../chromium/shell/features/boosts') =
    await loadModule('chromium/shell/features/boosts.ts', { fresh: true });

  instance++;

  const fire = (key: string, details: Navigation) => {
    const listener = listeners.get(key);

    if (!listener) throw new Error(`the shell registered no ${key} listener`);

    listener(details);
  };

  // The mocks resolve at once, so the shell is done when a turn of the event loop adds no call.
  const settle = async () => {
    for (let seen = -1; seen !== calls.length;) {
      seen = calls.length;
      await new Promise((resolve) => setImmediate(resolve));
    }
  };

  await boosts.ready();
  await settle();
  const take = () => calls.splice(0);

  return { boosts, fire, settle, take };
}

type Op =
  | { op: 'create'; host: string; kind: string }
  | { op: 'fileFor'; url: string | null; kind: string }
  | { op: 'save'; name: string; code: string; build?: CompiledBoost; reload?: boolean }
  | { op: 'remove'; name: string }
  | { op: 'list' }
  | { op: 'get'; name: string }
  | { op: 'boostedHosts' }
  | { op: 'commit'; tabId: number; frameId: number; url: string }
  | { op: 'domReady'; tabId: number; frameId: number; url: string; documentId: string };

type Shell = Awaited<ReturnType<typeof shell>>;

async function perform(s: Shell, op: Op): Promise<Json> {
  switch (op.op) {
    case 'create':
      // SAFETY: the shell's IPC handler passes the Boosts page's kind unchecked; 'module' tests create's own check.
      return plain(await s.boosts.create(op.host, op.kind as BoostKind));
    case 'fileFor':
      // SAFETY: as for create.
      return plain(await s.boosts.fileFor(op.url ?? undefined, op.kind as BoostKind));
    case 'save': return plain(await s.boosts.save(op.name, op.code, { build: op.build, reload: op.reload }));
    case 'remove': return plain(await s.boosts.remove(op.name));
    case 'list': return plain(await s.boosts.list());
    case 'get': return plain(s.boosts.get(op.name));
    case 'boostedHosts': return plain(s.boosts.boostedHosts());
    case 'commit':
      s.fire('committed', { tabId: op.tabId, frameId: op.frameId, url: op.url });

      return null;
    case 'domReady':
      s.fire('domReady', { tabId: op.tabId, frameId: op.frameId, url: op.url, documentId: op.documentId });

      return null;
  }
}

async function scenario(name: string, stored: Stored, tabs: Tab[], ops: Op[]) {
  const s = await shell(stored, tabs);
  const startup = s.take();
  const steps: Json[] = [];

  for (const op of ops) {
    const result = await perform(s, op);

    await s.settle();
    steps.push({ do: plain(op), result, calls: s.take() });
  }

  rows.push({ kind: 'scenario', name, stored: plain(stored), tabs: plain(tabs), startup, steps });
}

const tabs: Tab[] = [
  { id: 1, url: 'https://github.com/' },
  { id: 2, url: 'https://gist.github.com/x' },
  { id: 3, url: 'https://example.org/' },
  { id: 4, url: 'chrome://settings/' },
];

const createdHosts = [
  'github.com', ' GitHub.com ', 'WWW.github.com', 'www.www.example', 'all', 'a_b.example', 'a..b', '.a', 'a.', '-a.example',
  'bücher.example', 'xn--bcher-kva.example', '\u{212A}elvin.example', '\u{FEFF}feff.example', '\u{85}nel.example',
  '\u{A0}nbsp.example', '127.0.0.1', '[::1]', 'example.com:8080', '', 'www.', 'a b',
];

const pageUrls = [
  null, 'https://www.github.com/x', 'http://news.example/', 'HTTPS://up.example/', 'https://www./', 'file:///x', 'https://[::1]/',
  'https://bücher.example/', 'https://user@login.example:99/',
];

await scenario('create and name hosts', {}, tabs, [
  ...createdHosts.map((host): Op => ({ op: 'create', host, kind: 'styles' })),
  { op: 'create', host: 'github.com', kind: 'script' },
  { op: 'create', host: 'github.com', kind: 'script' },
  { op: 'create', host: 'github.com', kind: 'module' },
  ...pageUrls.map((url): Op => ({ op: 'fileFor', url, kind: 'script' })),
  { op: 'list' },
  { op: 'boostedHosts' },
]);

// parseBoostName takes this name, so the marker and sourceURL must hold it.
const hostileName = 'q"b\\c\t</script>\u{1}\u{7F}\u{1F600}.ts';

await scenario('save, apply and remove', {
  boosts: {
    'zeta.example.css': 'body { color: red }',
    'github.com.css': 'header { display: none }',
    'all.css': '* { outline: 0 }',
    'github.com.ts': 'mark()',
    'gist.github.com.ts': 'broken(',
    'Upper.example.ts': 'stored with an uppercase host',
    'example.org.ts': 'template only',
    'stray.example.txt': 'not a boost',
    // boostMatches takes any scheme. Chrome's scripting API kept this off chrome:// pages.
    'settings.css': 'body { zoom: 2 }',
    'settings.ts': 'mark()',
    [hostileName]: 'mark()',
    // list() sorts in UTF-16 order: the astral name sorts before U+E000, unlike code-point order.
    '\u{E000}private.css': 'p {}',
    '\u{1F600}astral.css': 'p {}',
  },
  boostsCompiled: {
    'github.com.ts': { code: '(() => {\nmark()\n})();' },
    'gist.github.com.ts': { error: 'Error transforming gist.github.com.ts: Unexpected token (1:7)' },
    'example.org.ts': {},
    'orphan.example.ts': { code: 'never run' },
    'settings.ts': { code: 'mark()' },
    [hostileName]: compileBoost(hostileName, 'mark()'),
  },
}, tabs, [
  { op: 'commit', tabId: 4, frameId: 0, url: 'chrome://settings/' },
  { op: 'domReady', tabId: 4, frameId: 0, url: 'chrome://settings/', documentId: 'doc-4' },
  { op: 'save', name: 'settings.css', code: 'body { zoom: 3 }' },
  { op: 'list' },
  { op: 'boostedHosts' },
  { op: 'get', name: 'github.com.ts' },
  { op: 'get', name: 'gist.github.com.ts' },
  { op: 'get', name: 'missing.example.ts' },
  { op: 'get', name: 'stray.example.txt' },
  { op: 'commit', tabId: 1, frameId: 0, url: 'https://github.com/' },
  { op: 'commit', tabId: 1, frameId: 3, url: 'https://github.com/frame' },
  { op: 'commit', tabId: 2, frameId: 0, url: 'https://gist.github.com/x' },
  { op: 'commit', tabId: 3, frameId: 0, url: 'https://example.org/' },
  { op: 'domReady', tabId: 1, frameId: 0, url: 'https://github.com/', documentId: 'doc-1' },
  { op: 'domReady', tabId: 1, frameId: 2, url: 'https://github.com/frame', documentId: 'doc-1f' },
  { op: 'domReady', tabId: 2, frameId: 0, url: 'https://gist.github.com/x', documentId: 'doc-2' },
  { op: 'domReady', tabId: 3, frameId: 0, url: 'https://example.org/', documentId: 'doc-3' },
  { op: 'domReady', tabId: 5, frameId: 0, url: 'https://upper.example/', documentId: 'doc-5' },
  { op: 'save', name: 'github.com.css', code: 'header { display: block }' },
  { op: 'save', name: 'github.com.ts', code: 'mark(2)', build: { code: '(() => {\nmark(2)\n})();' } },
  { op: 'save', name: 'github.com.ts', code: 'mark(3)', build: { code: '(() => {\nmark(3)\n})();' }, reload: false },
  { op: 'save', name: 'github.com.ts', code: 'oops(', build: { error: 'Error transforming github.com.ts: Unexpected token (1:6)' } },
  { op: 'domReady', tabId: 1, frameId: 0, url: 'https://github.com/', documentId: 'doc-1b' },
  { op: 'save', name: 'github.com.ts', code: '', build: { code: '', error: '' } },
  { op: 'get', name: 'github.com.ts' },
  { op: 'domReady', tabId: 1, frameId: 0, url: 'https://github.com/', documentId: 'doc-1c' },
  { op: 'save', name: 'new.example.ts', code: 'fresh' },
  { op: 'save', name: 'new.example.css', code: 'p {}', build: { code: 'ignored for styles' } },
  { op: 'save', name: 'Shout.EXAMPLE.ts', code: 'ok', build: { code: 'shout()' } },
  { op: 'save', name: 'a b.ts', code: 'rejected' },
  { op: 'save', name: 'under_score.ts', code: 'rejected' },
  { op: 'save', name: 'nope.js', code: 'rejected' },
  { op: 'save', name: 'zeta.example.css', code: 'body { color: blue }' },
  { op: 'remove', name: 'all.css' },
  { op: 'remove', name: 'all.css' },
  { op: 'remove', name: 'github.com.ts' },
  { op: 'remove', name: 'stray.example.txt' },
  { op: 'save', name: 'github.com.ts', code: 'back', build: { code: 'back()' } },
  { op: 'commit', tabId: 1, frameId: 0, url: 'https://github.com/' },
  { op: 'domReady', tabId: 1, frameId: 0, url: 'https://github.com/', documentId: 'doc-1d' },
  { op: 'list' },
  { op: 'boostedHosts' },
]);

// Chromium also ran registered user scripts, matched by its own URLPattern rules. These pages
// differ from boostMatches; tests/parity.rs checks the difference against url_pattern.cc's rules.
await scenario('registered user scripts', {
  boosts: { 'github.com.ts': 'mark()', 'Shout.EXAMPLE.ts': 'shout()', 'all.ts': 'every()' },
  boostsCompiled: {
    'github.com.ts': { code: 'mark()' },
    'Shout.EXAMPLE.ts': { code: 'shout()' },
    'all.ts': { code: 'every()' },
  },
}, [], [
  'https://github.com./', 'https://gist.github.com../', 'https://shout.example/', 'https://a.shout.example:8443/',
  'http://Shout.Example./', 'https://example.org/', 'https://notgithub.com/',
].map((url, i): Op => ({ op: 'domReady', tabId: 10 + i, frameId: 0, url, documentId: `doc-r${i}` })));

// ---- compiling, and what the compiled Boost does when a page runs it ---------

const compileCases = Object.entries({
  template: `// Boost for example.org: runs on every page of this site once it has loaded.
// It's TypeScript; Flecha strips the types when you save (⌘S), and open example.org tabs reload.

export {};

// document.querySelector('header')?.remove();
mark();
`,
  types: 'interface A { b: number }\nconst x: A = { b: 1 } as A;\nenum E { One }\nmark(x.b, E.One as number);\n',
  imports: "import x from 'y';\nexport const z = 1;\nexport default 3;\nmark();\n",
  'script-close': "const s = '</script><script>evil()</script>';\nmark(s);\n",
  'unbalanced-block-comment': 'mark(); /* never closed\n',
  'comment-closer': 'mark(); */ evil();\n',
  'line-comment-closer': '// })(); evil(); (() => {\nmark();\n',
  'string-closer': "mark('})(); evil(); (() => {');\n",
  'unterminated-template': 'mark(`open);\n',
  'template-closer': 'mark(`})(); evil(); (() => {${"`"}`);\n',
  'nested-template': 'mark(`a${`b${`c`}`}`);\n',
  'line-separator-string': "mark('\u{2028}\u{2029}');\n",
  'line-separator-comment': '// a\u{2028}})(); evil(); (() => {\nmark();\n',
  'paragraph-separator-comment': '// a\u{2029}})(); evil(); (() => {\nmark();\n',
  'extra-closer': 'mark(); })(); evil(); (() => {\n',
  // sucrase parses a module, where `<!--` is `<` `!` `--`. A page runs a classic script, where
  // Annex B reads `<!--` as a line comment.
  'html-open-comment': 'x = y <!--z; (() => {\n})(); evil(); (() => {\n0 <!--s; })();\nmark();\n',
  'html-open-comment-guard': 'x = y <!--z; if (1) { (() => {\n})(); } evil(); if (1) { (() => {\n0 <!--s; })(); }\nmark();\n',
  'html-close-comment': 'mark();\n--> })(); evil(); (() => {\n',
  'module-scope': 'var leaked = 1;\nfunction hoisted() {}\nmark(typeof exports, typeof module);\n',
  'reach-marker': "mark(Object.getOwnPropertySymbols(globalThis).map(String).join(','));\n",
  'use-strict-with': "'use strict';\nwith (globalThis) { mark(); }\n",
  'sloppy-with': 'with (globalThis) { mark(); }\n',
  'await-top': 'await 0;\nmark();\n',
  'source-url': '//# sourceURL=evil.js\nmark();\n',
  empty: '',
});

const boostName = (key: string) => `${key}.example.ts`;
const injectedBy = new Map<string, string>();

{
  const s = await shell({
    boosts: Object.fromEntries(compileCases.map(([key, source]) => [boostName(key), source])),
    boostsCompiled: Object.fromEntries(compileCases.map(([key, source]) => [boostName(key), compileBoost(boostName(key), source)])),
  }, []);

  for (const [key] of compileCases) s.fire('domReady', { tabId: 1, frameId: 0, url: `https://${key}.example/`, documentId: key });

  await s.settle();

  for (const call of s.take()) if (call.api === 'execute') injectedBy.set(call.documentId, call.code);
}

function sucrase(name: string, source: string) {
  try {
    return { code: transform(source, { transforms: ['typescript', 'imports'], filePath: name, production: true }).code };
  } catch (err) {
    return { error: err instanceof Error ? err.message : String(err) };
  }
}

/** Runs the injected Boost twice in one page, as both of the shell's injection paths can. */
function run(source: string) {
  const events: Json[] = [];

  const page = vm.createContext({
    mark: (...args: Array<string | number>) => void events.push(['mark', ...args.map(String)]),
    evil: () => void events.push(['evil']),
    y: 0,
  });

  const before = new Set(Reflect.ownKeys(page).map(String));
  const outcomes: Json[] = [];

  for (let i = 0; i < 2; i++) {
    try {
      new vm.Script(source, { filename: 'boost.js' }).runInContext(page);
      outcomes.push('ran');
    } catch (err) {
      // Errors from inside the context come from its own realm; String() gives "Name: message".
      // Only the name is kept, since the message is V8's wording and changes between Node releases.
      outcomes.push(String(err).split(':', 1)[0]);
    }
  }

  const globals = Reflect.ownKeys(page).map(String).filter((key) => !before.has(key)).sort();

  return { outcomes, events, globals };
}

for (const [key, source] of compileCases) {
  const name = boostName(key);
  const injected = injectedBy.get(key);

  rows.push({
    kind: 'compile',
    case: key,
    name,
    source,
    transform: sucrase(name, source),
    compiled: plain(compileBoost(name, source)),
    injected: injected ?? null,
    runs: injected ? run(injected) : null,
  });
}

{
  const name = 'large.example.ts';
  const line = 'void 0;\n';
  // A test input, not a limit: the shell sets no size limit on a Boost.
  const repeat = 512 * 1024;
  const big = line.repeat(repeat);
  const sha = (text: string) => createHash('sha256').update(text).digest('hex');
  const transformed = sucrase(name, big);
  const compiled = compileBoost(name, big);
  const s = await shell({ boosts: { [name]: big }, boostsCompiled: { [name]: compiled } }, []);
  let injected: string | undefined;

  for (const call of s.take()) if (call.api === 'register') injected = call.scripts[0].code;

  rows.push({
    kind: 'compile-large',
    name,
    line,
    repeat,
    sourceSha256: sha(big),
    transformSha256: transformed.code ? sha(transformed.code) : null,
    compiledSha256: compiled.code ? sha(compiled.code) : null,
    injectedSha256: injected ? sha(injected) : null,
  });
}

if (!instance) throw new Error('no shell instance ran');

await writeFile(OUT, rows.map((row) => JSON.stringify(row)).join('\n') + '\n');
