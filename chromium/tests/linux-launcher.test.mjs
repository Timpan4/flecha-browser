// Exercise independent checkout profiles against one shared executable without opening user browsers.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { execFileSync } from 'node:child_process';

async function checkout(dir, host) {
  await fs.mkdir(path.join(dir, 'chromium/scripts'), { recursive: true });
  await fs.mkdir(path.join(dir, 'chromium/shell/dist'), { recursive: true });
  await fs.mkdir(path.join(dir, 'chromium/native-host/dist'), { recursive: true });
  for (const name of ['run-linux.mjs', 'install-native-host.mjs']) {
    await fs.copyFile(`chromium/scripts/${name}`, path.join(dir, 'chromium/scripts', name));
  }
  await fs.writeFile(path.join(dir, 'chromium/shell/build.mjs'), '');
  await fs.writeFile(path.join(dir, 'chromium/native-host/build.mjs'), '');
  await fs.writeFile(path.join(dir, 'chromium/native-host/dist/flecha-host.cjs'), host);
}

test('Linux checkouts reuse one executable with distinct shells, profiles and helpers', { skip: process.platform !== 'linux' }, async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-launcher-'));
  try {
    const out = path.join(dir, 'shared');
    await fs.mkdir(out);
    await fs.writeFile(path.join(out, 'chrome'), '#!/usr/bin/env node\nconsole.log(JSON.stringify({ args: process.argv.slice(2), cwd: process.cwd(), launcher: process.env.__PYVENV_LAUNCHER__ }));\n', { mode: 0o755 });
    const run = (root, ...args) => execFileSync(process.execPath, [path.join(root, 'chromium/scripts/run-linux.mjs'), '--out', out, ...args], { cwd: os.tmpdir(), encoding: 'utf8', env: { ...process.env, __PYVENV_LAUNCHER__: 'invalid-test-launcher' }, stdio: ['ignore', 'pipe', 'pipe'] });
    for (const name of ['first', 'second']) {
      const root = path.join(dir, name);
      await checkout(root, name);
      const result = JSON.parse(run(root, '--', '--version').trim().split('\n').at(-1));
      assert.equal(result.cwd, root);
      assert.ok(result.args.includes(`--user-data-dir=${path.join(root, '.flecha-profile')}`));
      assert.ok(result.args.includes(`--flecha-shell-dir=${path.join(root, 'chromium/shell/dist')}`));
      assert.ok(result.args.includes('--version'));
      assert.equal(result.launcher, undefined);
      assert.equal(await fs.readFile(path.join(root, '.flecha-profile/native-host/flecha-host.cjs'), 'utf8'), name);
      const manifest = JSON.parse(await fs.readFile(path.join(root, '.flecha-profile/NativeMessagingHosts/dev.oprimo.flecha.host.json'), 'utf8'));
      assert.equal(manifest.path, path.join(root, '.flecha-profile/native-host/flecha-host'));
    }
    const first = path.join(dir, 'first');
    await fs.writeFile(path.join(first, '.flecha-profile/SingletonLock'), 'test-owned-lock');
    await fs.writeFile(path.join(first, '.flecha-profile/native-host/flecha-host.cjs'), 'keep-active-helper');
    assert.throws(() => run(first), /profile is locked/);
    assert.equal(await fs.readFile(path.join(first, '.flecha-profile/native-host/flecha-host.cjs'), 'utf8'), 'keep-active-helper');
    assert.throws(() => run(path.join(dir, 'second'), '--', '--user-data-dir=/unrelated-profile'), /Use --profile/);
    assert.throws(() => run(path.join(dir, 'second'), '--profile', 'relative'), /must be absolute/);
  } finally {
    await fs.rm(dir, { recursive: true, force: true });
  }
});
