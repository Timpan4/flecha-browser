import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { createHash } from 'node:crypto';
import { checkIdentity, compile, releaseArgs } from '../scripts/ci-release.mjs';
import { verifyRelease } from '../scripts/ci-publish.mjs';

test('release checkpoints reject another source, toolchain or platform', () => {
  const expected = { sha: 'source', chromium: '158', platform: 'linux-x64', depot: 'tools' };
  checkIdentity({ ...expected, complete: false }, expected);
  for (const key of Object.keys(expected)) {
    assert.throws(() => checkIdentity({ ...expected, complete: false, [key]: 'other' }, expected), /does not match/);
  }
  assert.throws(() => checkIdentity(expected, expected), /completion state/);
});

test('nightly GN configuration disables the stable updater and selects the target CPU', () => {
  const template = 'target_cpu = "arm64"\nflecha_shell_dir = "//flecha/shell"\nflecha_sparkle_feed_url = "https://flecha.dev/appcast.xml"\n';
  for (const platform of ['linux-x64', 'windows-x64', 'macos-arm64']) {
    const args = releaseArgs(template, { platform, version: '0.1.0', buildNumber: '1' });
    assert.doesNotMatch(args, /flecha_sparkle_/);
    assert.match(args, /use_siso = false/);
    assert.equal(args.includes('flecha_shell_dir'), platform.startsWith('macos-'));
    assert.ok(args.includes(`target_cpu = "${platform.startsWith('macos-') ? 'arm64' : 'x64'}"`));
  }
});

test('compilation errors cannot become resumable success', async () => {
  await assert.rejects(compile(process.execPath, ['-e', 'process.exit(7)'],
    {}), /Chromium build failed: 7/);
});

test('an intentional compilation deadline creates a checkpoint outcome', async () => {
  // An already-expired deadline exercises handoff without waiting for real compilation.
  assert.equal(await compile(process.execPath, ['-e', 'setInterval(() => {}, 1)'],
    { milliseconds: 0 }), false);
});

test('cancellation rejects rather than requesting a checkpoint', async () => {
  const controller = new AbortController();
  const result = compile(process.execPath, ['-e', 'setInterval(() => {}, 1)'],
    { signal: controller.signal });
  controller.abort(new Error('user cancelled'));
  await assert.rejects(result, /user cancelled/);
});

test('publication requires every platform, matching provenance and untampered packages', async t => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-release-test-'));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const platforms = [{ platform: 'linux-x64' }, { platform: 'windows-x64' }, { platform: 'macos-arm64' }];
  const expected = { sha: 'source', chromium: '158' };
  for (const { platform } of platforms) {
    const archive = `Flecha-${platform}.${platform.startsWith('windows-') ? 'zip' : 'tar.gz'}`;
    const files = { [archive]: 'package', [`${platform}.json`]: JSON.stringify({ ...expected, platform }) };
    let sums = '';
    for (const [name, content] of Object.entries(files)) {
      await fs.writeFile(path.join(dir, name), content);
      sums += `${createHash('sha256').update(content).digest('hex')}  ${name}\n`;
    }
    await fs.writeFile(path.join(dir, `${platform}-SHA256SUMS`), sums);
  }
  await verifyRelease(dir, platforms, expected);
  await assert.rejects(verifyRelease(dir, platforms, { ...expected, sha: 'other' }), /provenance mismatch/);
  await fs.writeFile(path.join(dir, 'Flecha-linux-x64.tar.gz'), 'corrupt');
  await assert.rejects(verifyRelease(dir, platforms, expected), /Checksum mismatch/);
  await fs.rm(path.join(dir, 'Flecha-windows-x64.zip'));
  await assert.rejects(verifyRelease(dir, [platforms[1]], expected), /Missing release asset/);
});
