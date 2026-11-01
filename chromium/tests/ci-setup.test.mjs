import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { cleanupXcodes } from '../scripts/ci-cleanup.mjs';

test('Xcode cleanup handles aliases without removing the selected SDK', async t => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-xcodes-'));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const old = path.join(dir, 'Xcode_26.0.1.app');
  const selected = path.join(dir, 'Xcode_26.6.app');
  const developer = path.join(selected, 'Contents/Developer');
  await fs.mkdir(old);
  await fs.mkdir(developer, { recursive: true });
  // Junctions do not require Windows developer mode or symlink privileges.
  await fs.symlink(old, path.join(dir, 'Xcode_26.0.app'), 'junction');
  await fs.symlink(selected, path.join(dir, 'Xcode.app'), 'junction');
  const canonicalDeveloper = await fs.realpath(developer);
  const aliasDeveloper = path.join(dir, 'Xcode.app/Contents/Developer');
  await cleanupXcodes(dir, aliasDeveloper, app => fs.rm(app, { recursive: true, force: true }));
  assert.equal(await fs.realpath(aliasDeveloper), canonicalDeveloper);
  await assert.rejects(fs.stat(old), { code: 'ENOENT' });
});

test('Windows Git checkout leaves patches applicable to Chromium LF sources', async t => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-patch-eol-'));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const repo = path.join(dir, 'repo');
  const checkout = path.join(dir, 'checkout');
  await fs.mkdir(path.join(repo, 'chromium/patches'), { recursive: true });
  let attributes = '';
  try {
    attributes = await fs.readFile(fileURLToPath(new URL('../../.gitattributes', import.meta.url)), 'utf8');
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
  // Chromium itself forces LF for Objective-C++ files on every platform.
  await fs.writeFile(path.join(repo, '.gitattributes'), `${attributes}\n*.mm text eol=lf\n`);
  await fs.writeFile(path.join(repo, 'target.mm'), 'before\n');
  const patch = 'chromium/patches/fixture.patch';
  await fs.writeFile(path.join(repo, patch), '--- a/target.mm\n+++ b/target.mm\n@@ -1 +1 @@\n-before\n+after\n');
  const git = (args, cwd = repo) => execFileSync('git', args, { cwd, stdio: 'pipe' });
  git(['init', '-q']);
  git(['add', '.']);
  git(['-c', 'user.name=Flecha test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'Patch fixture']);
  git(['-c', 'core.autocrlf=true', 'clone', '-q', '--local', repo, checkout], dir);
  git(['apply', patch], checkout);
  assert.equal(await fs.readFile(path.join(checkout, 'target.mm'), 'utf8'), 'after\n');
});
