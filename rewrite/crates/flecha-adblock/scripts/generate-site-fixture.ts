// Records the Chromium shell's per-site ad-block decisions for tests/site_parity.rs.
// Run from the repository root: bun rewrite/crates/flecha-adblock/scripts/generate-site-fixture.ts
import { blocksAdsOn, isSiteHost, withAdBlocking } from '../../../../chromium/shared/adBlockSites';
import { decodeState } from '../../../../chromium/shared/savedState';
import { fileURLToPath } from 'node:url';
import { z } from 'zod';

const urlRowSchema = z.strictObject({
  kind: z.literal('url'),
  url: z.string(),
  allowed: z.array(z.string()),
  tsHost: z.string().nullable(),
  tsBlocks: z.boolean(),
  // The decision with the entries isSiteHost refuses left out, as the rewrite keeps them.
  tsBlocksValidOnly: z.boolean(),
});

const hostRowSchema = z.strictObject({
  kind: z.literal('host'),
  host: z.string(),
  allowed: z.array(z.string()),
  tsSiteHost: z.boolean(),
  tsBlocks: z.boolean(),
  tsOff: z.array(z.string()),
  tsOn: z.array(z.string()),
});

const savedRowSchema = z.strictObject({
  kind: z.literal('saved'),
  value: z.unknown(),
  tsDecoded: z.array(z.string()).nullable(),
  tsValidEntries: z.array(z.string()).nullable(),
});

type Row = z.infer<typeof urlRowSchema> | z.infer<typeof hostRowSchema> | z.infer<typeof savedRowSchema>;

// Whitespace, invisible, case-folding and fullwidth characters that broke earlier ports.
const chars = ['﻿', '\u0085', '᠎', '​', '­', ' ', ' ', '　', 'ſ', 'K', '\t', '\n', '\r', ' ', '\u000B', ' ', '．', '。', 'ｅ', '%2e', '%20'];

const urlTemplates = ['{c}https://news.example/', 'https://{c}news.example/', 'https://news.example{c}/', 'https://ne{c}ws.example/', 'https://news.{c}example/', 'https://news.example/{c}', 'ht{c}tps://news.example/'];

const urls = [
  'https://news.example/',
  'https://NEWS.EXAMPLE/story',
  'https://www.news.example/',
  'https://deep.www.news.example/',
  'https://notnews.example/',
  'https://news.example.org/',
  'https://news.example./',
  'http://news.example:8080/x',
  'https://user:pw@news.example/',
  'https://[::1]/',
  'https://127.0.0.1/',
  'https://0x7f.1/',
  'https://2130706433/',
  'http://localhost:3000/',
  'https://xn--bcher-kva.example/',
  'https://bücher.example/',
  'https://BÜCHER.example/',
  'https://ſtory.example/',
  'https://Kexample.org/',
  'https://ｎｅｗｓ.example/',
  'https://news。example/',
  'https://%6eews.example/',
  'https://news%2eexample/',
  'file:///home/u/page.html',
  'about:blank',
  'data:text/html,hello',
  'foo://News.Example/',
  'blob:https://news.example/uuid',
  'not a url',
  '',
];

const allowedLists = [['news.example'], ['story.example', 'kexample.org', 'xn--bcher-kva.example'], ['127.0.0.1', 'localhost', 'news.example.'], []];

const hosts = [
  'news.example',
  'www.news.example',
  'notnews.example',
  'NEWS.example',
  'News.Example',
  'ſtory.example',
  'story.example',
  'Kexample.org',
  'kexample.org',
  'ｎｅｗｓ.example',
  'news..example',
  '.news.example',
  'news.example.',
  'news.example:80',
  '[::1]',
  '127.0.0.1',
  'localhost',
  'xn--bcher-kva.example',
  'bücher.example',
  '-news.example',
  'news_example',
  '',
];

const hostAllowedLists = [['news.example'], ['www.news.example', 'other.example'], ['story.example', 'kexample.org'], []];

const savedValues: unknown[] = [
  [],
  ['news.example'],
  ['news.example', 'news.example'],
  ['news.example', '[::1]'],
  ['Good.org', 'good.org'],
  [1, 'a.example'],
  [null],
  [['a.example']],
  ['ſtory.example', 'story.example'],
  ['Kexample.org'],
  ['ｎｅｗｓ.example', 'b.example'],
  [' news.example', 'news.example'],
  ['news.example\u0085'],
  ['news.example\t'],
  ['', 'a.example'],
  'news.example',
  {},
  null,
  7,
];

const urlInputs = new Set(urls);

for (const template of urlTemplates) for (const c of chars) urlInputs.add(template.replaceAll('{c}', c));

const hostInputs = new Set(hosts);

for (const c of chars) {
  hostInputs.add(c + 'news.example');
  hostInputs.add('news.example' + c);
  hostInputs.add('news.' + c + 'example');
}

function hostname(url: string) {
  try {
    return new URL(url).hostname;
  } catch {
    return null;
  }
}

const rows: Row[] = [];

for (const url of urlInputs) {
  for (const allowed of allowedLists) {
    const tsHost = hostname(url);
    // No hostname, no allow rule: declarativeNetRequest's requestDomains match hostnames only.
    const blocks = (sites: string[]) => tsHost === null || blocksAdsOn(sites, tsHost);
    rows.push({ kind: 'url', url, allowed, tsHost, tsBlocks: blocks(allowed), tsBlocksValidOnly: blocks(allowed.filter((site) => isSiteHost(site))) });
  }
}

for (const host of hostInputs) {
  for (const allowed of hostAllowedLists) {
    rows.push({
      kind: 'host',
      host,
      allowed,
      tsSiteHost: isSiteHost(host),
      tsBlocks: blocksAdsOn(allowed, host),
      tsOff: withAdBlocking(allowed, host, false),
      tsOn: withAdBlocking(allowed, host, true),
    });
  }
}

const minimalState = { tabs: {}, spaces: [{ id: 's', name: 'S' }] };

for (const value of savedValues) {
  const decoded = decodeState(JSON.stringify({ ...minimalState, adBlockAllowed: value }));

  if (!decoded) throw new Error(`The minimal saved state no longer decodes with ${JSON.stringify(value)}`);

  const entries = z.array(z.unknown()).safeParse(value);

  const tsValidEntries = entries.success
    ? entries.data.flatMap((entry) => {
        const host = z.string().safeParse(entry);

        return host.success && isSiteHost(host.data) ? [host.data] : [];
      })
    : null;

  rows.push({ kind: 'saved', value, tsDecoded: decoded.adBlockAllowed ?? null, tsValidEntries });
}

const kinds: Record<Row['kind'], number> = { url: 0, host: 0, saved: 0 };

for (const row of rows) kinds[row.kind] += 1;

type FixtureLine = Row | { counts: Record<Row['kind'], number> };

const asciiJson = (value: FixtureLine) =>
  JSON.stringify(value).replace(/[\u007F-￿]/g, (unit) => '\\u' + unit.charCodeAt(0).toString(16).padStart(4, '0'));

const fixturePath = fileURLToPath(new URL('../tests/fixtures/site-parity.jsonl', import.meta.url));

await Bun.write(fixturePath, [asciiJson({ counts: kinds }), ...rows.map(asciiJson)].join('\n') + '\n');
