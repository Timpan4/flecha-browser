
import { suggestable } from '../../../../chromium/shell/lib/suggest';
import { resolveInput } from '../../../../chromium/shell/lib/url';

type Divergence = { reason: string; url?: string; suggestable?: boolean };

type AddressCase = { input: string; tsUrl: string; tsSuggestable: boolean; rust?: Divergence };

// Cross-product corpus from independent verification of the command bar.
const chars = ['﻿','\u0085','᠎','​','‌','‍','⁠','­',' ',' ','　','ſ','K','\t','\n',' ','\u000B',' ','͏','ᅟ','ﾠ','ㅤ','\u{E0001}','\u{1D173}'];

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

for (const t of templates) for (const c of chars) inputs.add(t.replaceAll('{c}', c));

for (const t of templates) inputs.add(t.replaceAll('{c}', ''));

for (const input of [
  '\u0085host.example\u0085/p?token=x#private',
  'host.\u0085example/p?token=x#private',
  'host.example\u0085:8443/private?token=x',
  '​https:///p?token=x',
  'ht­tps:///p?token=x',
  '\u0085https://host.example/p?token=x',
  'ht\u0085tps://host.example/p?token=x',
  '​ht﻿tps://host.example/p?token=x',
  'ht⁠tp://host.example/p?token=x',
  '⁠file:///home/u/secret',
  '᠎ftp://host.example/p?token=x',
  'https://',
  'https:///p?token=x',
  'ht\ttps:///p?token=x',
]) inputs.add(input);

const fixture = Bun.file(new URL('../tests/fixtures/address-parity.jsonl', import.meta.url));
const divergences = new Map<string, Divergence>();

if (await fixture.exists()) {
  for (const line of (await fixture.text()).split('\n').filter(Boolean)) {
    // SAFETY: tests/address_parity.rs deserializes every fixture row into this same shape with serde, so a
    // malformed or hand-edited row fails the Rust test before regeneration can rely on it.
    const prior = JSON.parse(line) as AddressCase;

    if (prior.rust) divergences.set(prior.input, prior.rust);
  }
}

const rows = Array.from(inputs, (input): AddressCase => {
  const rust = divergences.get(input);
  divergences.delete(input);

  return { input, tsUrl: resolveInput(input, 'google'), tsSuggestable: suggestable(input), ...(rust && { rust }) };
});

if (divergences.size) throw new Error('A documented Rust divergence is missing from the corpus');

const asciiJson = (row: AddressCase) =>
  JSON.stringify(row).replace(/[\u007F-￿]/g, (unit) => '\\u' + unit.charCodeAt(0).toString(16).padStart(4, '0'));

await Bun.write(fixture, rows.map(asciiJson).join('\n') + '\n');
