// The shell's manifest version, from the built files and the manifest. Chromium keeps running the
// background script it registered until the extension's version changes (or Chromium's own
// version, which a Flecha rebuild keeps), so a new shell under an unchanged version would never
// reach a profile that already ran an older one. Same shell, same version; any change, a new one.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

// Chromium writes _metadata into the shell folder it loads; the manifest is hashed apart, without
// the version it gets.
const skipped = new Set(['manifest.json', '_metadata']);

function files(dir, rel = '') {
  return fs
    .readdirSync(path.join(dir, rel), { withFileTypes: true })
    .filter((e) => !(rel === '' && skipped.has(e.name)))
    .flatMap((e) => (e.isDirectory() ? files(dir, path.join(rel, e.name)) : [path.join(rel, e.name)]))
    .sort();
}

// "0.1.<a>.<b>": the manifest's first two version parts, then 32 bits of the hash (Chromium
// allows 0–65535 per part).
export function shellVersion(dir, manifest) {
  const { version, ...rest } = manifest;
  const hash = crypto.createHash('sha256').update(JSON.stringify(rest)).update('\0');
  for (const f of files(dir)) hash.update(f).update('\0').update(fs.readFileSync(path.join(dir, f))).update('\0');
  const digest = hash.digest();
  const base = version.split('.').slice(0, 2).join('.');
  return `${base}.${digest.readUInt16BE(0)}.${digest.readUInt16BE(2)}`;
}

// Writes the source manifest into dir with the version of what dir holds. Run it again whenever
// anything in dir changes.
export function writeManifest(source, dir) {
  const manifest = JSON.parse(fs.readFileSync(source, 'utf8'));
  manifest.version = shellVersion(dir, manifest);
  fs.writeFileSync(path.join(dir, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
}
