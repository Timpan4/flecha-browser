import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { checkIdentity, compile, configureReleaseTree, depotRevision, releaseConfiguration, saveCheckpoint, restoreCheckpoint } from '../scripts/ci-release.mjs';
import { findReusableArtifact, nativeKey, reusableArtifact } from '../scripts/ci-reuse.mjs';
import { verifyRelease } from '../scripts/ci-publish.mjs';

test('native reuse ignores shell-only commits and invalidates native inputs and toolchains', async t => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-native-key-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  await fs.mkdir(path.join(root, 'chromium/patches'), { recursive: true });
  await fs.mkdir(path.join(root, 'chromium/shell'));
  await fs.writeFile(path.join(root, 'chromium/patches/native.patch'), 'native input');
  await fs.writeFile(path.join(root, 'chromium/shell/app.ts'), 'old shell');
  execFileSync('git', ['init', '--quiet', root]);
  const commit = () => {
    execFileSync('git', ['add', '.'], { cwd: root });
    execFileSync('git', ['-c', 'user.name=Release test', '-c', 'user.email=release@example.invalid',
      'commit', '--quiet', '-m', 'fixture'], { cwd: root });
  };
  commit();
  const configuration = { platform: 'linux-x64', depot: 'toolchain', args: 'release flags', work: '/stable/work' };
  const original = nativeKey(root, configuration);
  await fs.writeFile(path.join(root, 'chromium/shell/app.ts'), 'new shell');
  commit();
  assert.equal(nativeKey(root, configuration), original);
  for (const field of Object.keys(configuration)) {
    assert.notEqual(nativeKey(root, { ...configuration, [field]: 'changed' }), original, field);
  }
  await fs.writeFile(path.join(root, 'chromium/patches/native.patch'), 'changed native input');
  commit();
  assert.notEqual(nativeKey(root, configuration), original);
});

test('recovery searches trusted nightly runs and skips expired or superseded checkpoints', () => {
  const options = { repository: 'owner/repo', branch: 'main', currentRun: '10', platform: 'linux-x64', key: 'native-key' };
  const run = (id, overrides = {}) => ({ id, head_sha: `sha-${id}`, head_repository: { full_name: options.repository },
    path: '.github/workflows/nightly-release.yml', head_branch: 'main', event: 'workflow_dispatch', ...overrides });
  const requests = [];
  const rejected = new Map([
    ['9', { head_branch: 'feature' }], ['8', { head_repository: { full_name: 'fork/repo' } }],
    ['7', { event: 'pull_request' }], ['6', { path: '.github/workflows/untrusted.yml' }],
  ]);
  const candidate = findReusableArtifact(options, endpoint => {
    requests.push(endpoint);
    if (endpoint.endsWith('page=1')) return { artifacts: ['10', ...rejected.keys()].map(id =>
      ({ name: 'native-linux-x64-native-key', workflow_run: { id } })), total_count: 9 };
    if (endpoint.endsWith('page=2')) return { artifacts: [
      { name: 'reuse-linux-x64-native-key-2' }, { name: 'reuse-linux-x64-native-key-1' },
    ].map(artifact => ({ ...artifact, workflow_run: { id: 5 } })), total_count: 9 };
    if (endpoint.endsWith('page=3')) return { artifacts: [
      { name: 'checkpoint-linux-x64-2', expired: true }, { name: 'checkpoint-linux-x64-1' },
    ].map(artifact => ({ ...artifact, workflow_run: { id: 5 } })), total_count: 9 };
    const id = endpoint.split('/').at(-1);
    return run(Number(id), rejected.get(id));
  });
  assert.deepEqual(candidate, { run: '5', sha: 'sha-5', name: 'checkpoint-linux-x64-1', archive: 'checkpoint.tar.gz' });
  assert.equal(requests.length, 8);
  assert.equal(reusableArtifact([{ name: 'reuse-linux-x64-other-key-1' }, { name: 'checkpoint-linux-x64-1' }], options.platform, options.key), undefined);
  assert.deepEqual(reusableArtifact([{ name: 'native-linux-x64-native-key' }], options.platform, options.key),
    { name: 'native-linux-x64-native-key', archive: 'native.tar.gz' });
  assert.equal(findReusableArtifact(options, () => ({ artifacts: [], total_count: 0 })), undefined);
});

test('a restored native tree retains compiler outputs and adopts current shell, build metadata and source provenance', async t => {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-reconfigure-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const original = path.join(root, 'original');
  const work = path.join(root, 'restored work');
  const src = path.join(work, 'src');
  const out = path.join(src, 'out/Release');
  await fs.mkdir(path.join(original, 'src/out/Release'), { recursive: true });
  await fs.mkdir(path.join(original, 'depot_tools'));
  await fs.mkdir(path.join(original, 'src/flecha/shell'), { recursive: true });
  await fs.mkdir(path.join(root, 'chromium/shell/dist'), { recursive: true });
  const identity = { sha: 'current-source', chromium: '158', platform: 'linux-x64', depot: 'tools' };
  await fs.writeFile(path.join(original, 'state.json'), JSON.stringify({ ...identity, sha: 'previous-source', nativeKey: 'key', complete: true }));
  await fs.writeFile(path.join(original, 'src/out/Release/chrome.o'), 'retained compiler output');
  await fs.writeFile(path.join(original, 'src/flecha/shell/removed.js'), 'stale shell');
  await fs.writeFile(path.join(root, 'chromium/shell/dist/current.js'), 'current shell');
  const archive = path.join(root, 'native.tar.gz');
  saveCheckpoint(original, archive, false);
  await restoreCheckpoint(work, archive);
  let generated = false;
  const reused = { sha: 'previous-source', run: '123' };
  const configure = { root, work, identity, key: 'key', reused, args: 'current version and build number', generate: () => { generated = true; } };
  await assert.rejects(configureReleaseTree({ ...configure, key: 'wrong' }), /nativeKey does not match/);
  assert.equal(generated, false);
  await configureReleaseTree(configure);
  assert.equal(generated, true);
  assert.equal(await fs.readFile(path.join(out, 'chrome.o'), 'utf8'), 'retained compiler output');
  assert.equal(await fs.readFile(path.join(out, 'args.gn'), 'utf8'), 'current version and build number');
  assert.equal(await fs.readFile(path.join(src, 'flecha/shell/current.js'), 'utf8'), 'current shell');
  await assert.rejects(fs.stat(path.join(src, 'flecha/shell/removed.js')), { code: 'ENOENT' });
  const state = JSON.parse(await fs.readFile(path.join(work, 'state.json'), 'utf8'));
  checkIdentity(state, identity);
  assert.equal(state.complete, false);
  assert.deepEqual(state.reusedFrom, reused);
});

test('completed platforms pass through later build and checkpoint CLI stages without source', async t => {
  // Node resolves module filenames through macOS's /var -> /private/var alias.
  const dir = await fs.realpath(await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-completed-')));
  t.after(() => fs.rm(dir, { recursive: true, force: true }));
  const root = path.join(dir, 'release tooling');
  const scripts = path.join(root, 'chromium/scripts');
  await fs.mkdir(scripts, { recursive: true });
  const script = path.join(scripts, 'ci-release.mjs');
  await fs.copyFile(fileURLToPath(new URL('../scripts/ci-release.mjs', import.meta.url)), script);
  await fs.copyFile(fileURLToPath(new URL('../scripts/ci-reuse.mjs', import.meta.url)), path.join(scripts, 'ci-reuse.mjs'));
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

test('nightly preparation selects platform GN settings and the required PGO checkout', async () => {
  for (const platform of ['linux-x64', 'windows-x64', 'macos-arm64']) {
    const { args, gclient } = await releaseConfiguration({ platform, version: '0.1.0', buildNumber: '1' });
    assert.doesNotMatch(args, /flecha_sparkle_/);
    assert.match(args, /use_siso = false/);
    assert.match(args, /proprietary_codecs = false/);
    assert.equal(args.includes('flecha_shell_dir'), platform.startsWith('macos-'));
    assert.ok(args.includes(`target_cpu = "${platform.startsWith('macos-') ? 'arm64' : 'x64'}"`));
    assert.match(args, /flecha_version = "0\.1\.0"/);
    assert.match(args, /flecha_build_number = "1"/);
    if (platform === 'linux-x64') {
      assert.match(args, /is_official_build = true/);
      assert.match(args, /chrome_pgo_phase = 2/);
      assert.match(gclient, /"checkout_pgo_profiles": True/);
      assert.doesNotMatch(args, /ffmpeg_branding = "Chrome"/);
    } else {
      assert.match(args, /chrome_pgo_phase = 0/);
      assert.match(gclient, /"checkout_pgo_profiles": False/);
    }
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
