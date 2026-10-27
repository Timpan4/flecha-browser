// The shell's manifest version, from the built files. Chromium keeps running the background
// script it registered until the extension's version changes (or Chromium's own version, which
// a Flecha rebuild keeps), so a new shell under an unchanged version would never reach a profile
// that already ran an older one. Same files, same version; any change, a new one.
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';

// Chromium writes _metadata into the shell folder it loads; the manifest carries the version.
const skipped = new Set(['manifest.json', '_metadata']);

function files(dir, rel = '') {
  return fs
    .readdirSync(path.join(dir, rel), { withFileTypes: true })
    .filter((e) => !(rel === '' && skipped.has(e.name)))
    .flatMap((e) => (e.isDirectory() ? files(dir, path.join(rel, e.name)) : [path.join(rel, e.name)]))
    .sort();
}

// "0.1.<a>.<b>": the base version, then 32 bits of the files' hash (Chromium allows 0–65535 per part).
export function shellVersion(dir, base = '0.1') {
  const hash = crypto.createHash('sha256');
  for (const f of files(dir)) hash.update(f).update('\0').update(fs.readFileSync(path.join(dir, f))).update('\0');
  const digest = hash.digest();
  return `${base}.${digest.readUInt16BE(0)}.${digest.readUInt16BE(2)}`;
}

// Writes the source manifest into dir with the version of what dir holds.
export function writeManifest(source, dir) {
  const manifest = JSON.parse(fs.readFileSync(source, 'utf8'));
  manifest.version = shellVersion(dir, manifest.version.split('.').slice(0, 2).join('.'));
  fs.writeFileSync(path.join(dir, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
}
