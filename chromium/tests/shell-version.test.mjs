// A rebuilt shell gets a new manifest version, so Chromium registers its new background script
// instead of running the one an earlier install left in the profile.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { test } from 'node:test';
import { shellVersion, writeManifest } from '../shell/build-version.mjs';

const manifest = { name: 'Flecha', version: '0.1.0', background: { service_worker: 'background.js' } };

function shell(files) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'flecha-shell-'));
  for (const [f, text] of Object.entries(files)) {
    fs.mkdirSync(path.dirname(path.join(dir, f)), { recursive: true });
    fs.writeFileSync(path.join(dir, f), text);
  }
  return dir;
}

test('the same files get the same version, a changed file a new one', () => {
  const before = shellVersion(shell({ 'background.js': 'old', 'icons/16.png': 'icon' }), manifest);
  assert.equal(shellVersion(shell({ 'background.js': 'old', 'icons/16.png': 'icon' }), manifest), before);
  assert.notEqual(shellVersion(shell({ 'background.js': 'new', 'icons/16.png': 'icon' }), manifest), before);
});

test('a manifest change alone, such as another background script, gets a new version', () => {
  const dir = shell({ 'background.js': 'one', 'worker.js': 'two' });
  const moved = { ...manifest, background: { service_worker: 'worker.js' } };
  assert.notEqual(shellVersion(dir, moved), shellVersion(dir, manifest));
});

test('what Chromium writes into the shell folder leaves the version alone', () => {
  const dir = shell({ 'background.js': 'code' });
  const before = shellVersion(dir, manifest);
  fs.mkdirSync(path.join(dir, '_metadata'));
  fs.writeFileSync(path.join(dir, '_metadata/ruleset'), 'indexed');
  assert.equal(shellVersion(dir, manifest), before);
});

test('the written manifest keeps the base version and is one Chromium accepts', () => {
  const dir = shell({ 'background.js': 'code' });
  const source = path.join(shell({ 'manifest.json': JSON.stringify(manifest) }), 'manifest.json');
  writeManifest(source, dir);
  const { name, version } = JSON.parse(fs.readFileSync(path.join(dir, 'manifest.json'), 'utf8'));
  assert.equal(name, 'Flecha');
  assert.match(version, /^0\.1\.\d+\.\d+$/);
  for (const part of version.split('.')) assert.ok(Number(part) <= 65535 && String(Number(part)) === part);
  writeManifest(source, dir);
  assert.equal(JSON.parse(fs.readFileSync(path.join(dir, 'manifest.json'), 'utf8')).version, version);
});

test('restamping a built shell from its own manifest gives the version a full build would', () => {
  const dir = shell({ 'background.js': 'code' });
  const source = path.join(shell({ 'manifest.json': JSON.stringify(manifest) }), 'manifest.json');
  writeManifest(source, dir);
  fs.writeFileSync(path.join(dir, 'rules.json'), '[]');
  writeManifest(path.join(dir, 'manifest.json'), dir);
  const restamped = JSON.parse(fs.readFileSync(path.join(dir, 'manifest.json'), 'utf8'));
  writeManifest(source, dir);
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(dir, 'manifest.json'), 'utf8')), restamped);
});
