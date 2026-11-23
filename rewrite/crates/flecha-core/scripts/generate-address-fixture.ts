
import { suggestable } from '../../../../chromium/shell/lib/suggest';
import { resolveInput } from '../../../../chromium/shell/lib/url';
import { fileURLToPath } from 'node:url';
import { z } from 'zod';

const divergenceSchema = z.strictObject({ reason: z.string().min(1), url: z.string().optional(), suggestable: z.boolean().optional() });

const addressCaseSchema = z.strictObject({
  input: z.string(),
  tsUrl: z.string(),
  tsSuggestable: z.boolean(),
  note: z.string().min(1).optional(),
  rust: divergenceSchema.optional(),
});

type Divergence = z.infer<typeof divergenceSchema>;

type AddressCase = z.infer<typeof addressCaseSchema>;

// Cross-product corpus from independent verification of the command bar.
const chars = ['\ufeff','\u0085','\u180e','\u200b','\u200c','\u200d','\u2060','\u00ad','\u2028','\u00a0','\u3000','\u017f','\u212a','\t','\n',' ','\u000B','\u1680','\u034f','\u115f','\uffa0','\u3164','\u{E0001}','\u{1D173}'];

const templates = [
 '{c}https://host.example/p?token=x','https://host.example/p?token=x{c}','ht{c}tps://host.example/p?token=x','https{c}://host.example/p?token=x',
 'https:{c}//host.example/p?token=x','https://{c}host.example/p?token=x','https://host.example{c}/p?token=x','https://ho{c}st.example/p?token=x',
 'host.example{c}/p?token=x','{c}host.example/p?token=x','ho{c}st.example/p?token=x','host{c}.example/p?token=x','host.{c}example/p?token=x',
 'user:pa{c}ss@host.example/p','user:pass@{c}host.example/p','us{c}er@host.example/p?x=1','host.example:{c}8080/p?token=x','host.example{c}:8080/p?token=x',
 'localhost{c}:3000/p?token=x','{c}localhost:3000/p?token=x','192.168.0.1{c}/p?token=x','{c}192.168.0.1/admin?token=x','192.168{c}.0.1/admin?token=x',
 '{c}file:///home/u/secret','{c}/home/u/secret','{c}http://intranet/p?token=x','http://{c}/p?token=x','{c}ftp://h.example/p?token=x',
 '{c}mailto:a@b.example','{c}{c}https://host.example/p?token=x','{c}HTTPS://HOST.EXAMPLE/P?TOKEN=X','https://host.example/p?to{c}ken=x',
 'rust{c}1.2.3 release notes','rust 1.2.3 release notes{c}','{c}example.com is down','10:30{c} meeting','{c}1/2 cup',
 'notes {c}host.example/p?token=x','see {c}https://host.example/p?token=x now','{c}[::1]:8080/p?token=x','{c}127.0.0.1:8080/p',
];

const inputs = new Set<string>();

const notes = new Map([
  ['example.com../p?token=x', 'Only one DNS root dot is ignored. The extra dot leaves the path local.'],
  ['localhost..', 'Only one DNS root dot is ignored. A host with no path can fall back to search.'],
]);

for (const t of templates) for (const c of chars) inputs.add(t.replaceAll('{c}', c));

for (const t of templates) inputs.add(t.replaceAll('{c}', ''));

for (const input of [
  '\u0085host.example\u0085/p?token=x#private',
  'host.\u0085example/p?token=x#private',
  'host.example\u0085:8443/private?token=x',
  '\u200Bhttps:///p?token=x',
  'ht\u00ADtps:///p?token=x',
  '\u0085https://host.example/p?token=x',
  'ht\u0085tps://host.example/p?token=x',
  '\u200Bht\uFEFFtps://host.example/p?token=x',
  'ht\u2060tp://host.example/p?token=x',
  '\u2060file:///home/u/secret',
  '\u180Eftp://host.example/p?token=x',
  'https://',
  'https:///p?token=x',
  'ht\ttps:///p?token=x',
]) inputs.add(input);

for (const input of [...notes.keys(), 'ftp://', 'ws://', 'wss://']) inputs.add(input);

// Labels that URL host parsing reads as IPv4 or rejects as punycode, followed by a port and a private tail.
const hostLabels = ['xn--', 'xn--a', 'xn--zz', 'XN--ZZ', 'xn---', 'xn--ls8h', 'xn--abc-', 'xn--0', 'xn--a-ecp', '0x', '0x0', '0xa', '0x1f', '0X1F', '0xff', '0x7f000001', '0xdeadbeef', '0xffffffff', '0x100000000', '0x1ffffffff', '0b1', '0o7'];

const portTails = [':80/reset?token=abc', ':8080/p', ':80?token=x', ':3000/api#k'];

for (const label of hostLabels) for (const tail of portTails) inputs.add(label + tail);

const fixturePath = fileURLToPath(new URL('../tests/fixtures/address-parity.jsonl', import.meta.url));
const fixture = Bun.file(fixturePath);
const divergences = new Map<string, Divergence>();

if (await fixture.exists()) {
  for (const [index, line] of (await fixture.text()).split('\n').entries()) {
    if (!line) continue;

    try {
      const prior = addressCaseSchema.parse(JSON.parse(line));

      if (prior.rust) divergences.set(prior.input, prior.rust);
    } catch (cause) {
      throw new Error(`${fixturePath}:${index + 1}: invalid address fixture row`, { cause });
    }
  }
}

const rows = Array.from(inputs, (input): AddressCase => {
  const rust = divergences.get(input);
  const note = notes.get(input);
  divergences.delete(input);

  return { input, tsUrl: resolveInput(input, 'google'), tsSuggestable: suggestable(input), ...(note && { note }), ...(rust && { rust }) };
});

if (divergences.size) throw new Error('A documented Rust divergence is missing from the corpus');

const asciiJson = (row: AddressCase) =>
  JSON.stringify(row).replace(/[\u007F-\uFFFF]/g, (unit) => '\\u' + unit.charCodeAt(0).toString(16).padStart(4, '0'));

await Bun.write(fixture, rows.map(asciiJson).join('\n') + '\n');
