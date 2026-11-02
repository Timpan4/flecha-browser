import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { checkIdentity, compile, depotRevision, releaseArgs, saveCheckpoint, restoreCheckpoint } from '../scripts/ci-release.mjs';
import { verifyRelease } from '../scripts/ci-publish.mjs';

test('completed platforms pass through later build and checkpoint CLI stages without source', async t => {
  // Node resolves module filenames through macOS's /var -> /private/var alias.
  const dir = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-completed-')));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const root = path.join(dir, 'release tooling');
  const scripts = path.join(root, 'chromium/scripts');
  await fs.mkdir(scripts, { recursive: true });
  const script = path.join(scripts, 'ci-release.mjs');
  await fs.copyFile(fileURLToPath(new URL('../scripts/ci-release.mjs', import.meta.url)), script);
  await fs.writeFile(path.join(root, 'chromium/CHROMIUM_VERSION'), 'fixture');
  execFileSync('git', ['init', '--quiet', root]);
  execFileSync('git', ['-c', 'user.name=Release test', '-c', 'user.email=release@example.invalid',
    'commit', '--allow-empty', '-m', 'Release fixture'], { cwd: root });
  const sha = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
  const platform = process.platform === 'darwin' ? 'macos-arm64' : process.platform === 'win32' ? 'windows-x64' : 'linux-x64';
  const state = { sha, chromium: 'fixture', platform, depot: depotRevision, complete: true };
  let workspace = path.join(dir, 'first workspace');
  const work = path.join(workspace, 'flecha-chromium');
  await fs.mkdir(work, { recursive: true });
  await fs.writeFile(path.join(work, 'state.json'), JSON.stringify(state));
  for (const stage of ['first', 'second']) {
    const env = { ...process.env, GITHUB_ACTIONS: 'true', RUNNER_ENVIRONMENT: 'github-hosted', RUNNER_WORKSPACE: workspace };
    execFileSync(process.execPath, [script, 'build'], { env });
    execFileSync(process.execPath, [script, 'checkpoint'], { env });
    assert.equal((await fs.stat(path.join(root, 'checkpoint.tar.gz'))).isFile(), true);
    const next = path.join(dir, `${stage} restored workspace`);
    await fs.mkdir(next);
    const restored = path.join(next, 'flecha-chromium');
    await restoreCheckpoint(restored, path.join(root, 'checkpoint.tar.gz'));
    assert.deepEqual(JSON.parse(await fs.readFile(path.join(restored, 'state.json'), 'utf8')), state);
    await assert.rejects(fs.stat(path.join(restored, 'src')), { code: 'ENOENT' });
    workspace = next;
  }
});

test('checkpoint archives restore the tree, and completed checkpoints carry only state', async t => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-checkpoint-'));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const work = path.join(dir, 'work tree');
  await fs.mkdir(path.join(work, 'src/out/Release'), { recursive: true });
  await fs.writeFile(path.join(work, 'state.json'), '{"complete":false}');
  await fs.writeFile(path.join(work, 'src/out/Release/chrome'), 'compiled object');
  for (const complete of [false, true]) {
    const archive = path.join(dir, `checkpoint-${complete}.tar.gz`);
    const restored = path.join(dir, `restored tree-${complete}`);
    saveCheckpoint(work, archive, complete);
    await restoreCheckpoint(restored, archive);
    await assert.rejects(fs.stat(archive), { code: 'ENOENT' });
    assert.equal(await fs.readFile(path.join(restored, 'state.json'), 'utf8'), '{"complete":false}');
    const binary = path.join(restored, 'src/out/Release/chrome');
    if (complete) await assert.rejects(fs.stat(binary), { code: 'ENOENT' });
    else assert.equal(await fs.readFile(binary, 'utf8'), 'compiled object');
  }
  await t.test('restore preserves dangling links and links archived before their targets', async t => {
    try {
      await fs.symlink('src/out/Release/chrome', path.join(work, 'a-forward-link'), 'file');
      await fs.symlink('src/out/Release', path.join(work, 'a-directory-link'), 'dir');
      await fs.symlink('missing-target', path.join(work, 'dangling-link'), 'file');
    } catch (error) {
      if (process.platform === 'win32' && process.env.GITHUB_ACTIONS !== 'true' && error.code === 'EPERM') {
        t.skip('Local Windows account lacks native symlink privileges. Hosted CI requires this test.');
        return;
      }
      throw error;
    }
    const archive = path.join(dir, 'links-first.tar.gz');
    const restored = path.join(dir, 'restored links');
    const tar = process.platform === 'win32' ? path.join(process.env.ProgramFiles, 'Git/usr/bin/tar.exe') : 'tar';
    const flags = process.platform === 'win32' ? ['--force-local'] : [];
    const archivePath = archive.replaceAll(path.sep, '/');
    const links = ['a-directory-link', 'a-forward-link', 'dangling-link'];
    // Explicit arguments control member order, independent of filesystem enumeration.
    execFileSync(tar, [...flags, '-czf', archivePath, ...links, 'src'], { cwd: work });
    const members = execFileSync(tar, [...flags, '-tzf', archivePath], { encoding: 'utf8' }).trim().split(/\r?\n/);
    assert.deepEqual(members.slice(0, links.length), links);
    await restoreCheckpoint(restored, archive);
    for (const link of links) {
      assert.equal((await fs.lstat(path.join(restored, link))).isSymbolicLink(), true, link);
    }
    assert.equal(await fs.readFile(path.join(restored, 'a-forward-link'), 'utf8'), 'compiled object');
    assert.equal(await fs.readFile(path.join(restored, 'a-directory-link/chrome'), 'utf8'), 'compiled object');
    await assert.rejects(fs.stat(path.join(restored, 'dangling-link')), { code: 'ENOENT' });
  });
});

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
