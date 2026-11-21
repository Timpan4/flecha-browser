import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { checkpointPath, depotRevision, saveCheckpoint } from '../scripts/ci-release.mjs';
import { verifyRelease } from '../scripts/ci-publish.mjs';

test('Linux stage zero falls through to a warm native tree on miss and emits a verified current package on hit',
  { skip: process.platform !== 'linux' && 'Linux runner fixture' }, async t => {
    const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-stage-zero-'));
    t.after(() => fs.rm(dir, { recursive: true, force: true }));
    const root = path.join(dir, 'checkout');
    const workspace = path.join(dir, 'runner');
    const work = path.join(workspace, 'flecha-chromium');
    const bin = path.join(dir, 'bin');
    const seed = path.join(dir, 'seed');
    for (const folder of [workspace, bin, seed, ...['chromium/scripts', 'chromium/build', 'chromium/shell/dist',
      'chromium/native-host/dist'].map(folder => path.join(root, folder))]) await fs.mkdir(folder, { recursive: true });
    for (const name of ['ci-release.mjs', 'ci-reuse.mjs', 'ci-publish.mjs', 'portable-launch.mjs']) {
      await fs.copyFile(new URL('../scripts/' + name, import.meta.url), path.join(root, 'chromium/scripts', name));
    }
    await fs.copyFile(new URL('../build/linux-release.gn', import.meta.url), path.join(root, 'chromium/build/linux-release.gn'));
    await fs.writeFile(path.join(root, 'package.json'), '{"version":"1.0.0"}');
    await fs.writeFile(path.join(root, 'chromium/CHROMIUM_VERSION'), '158');
    await fs.writeFile(path.join(root, 'chromium/shell/dist/shell.js'), 'current shell');
    await fs.writeFile(path.join(root, 'chromium/native-host/dist/flecha-host.cjs'), 'current host');
    await fs.writeFile(path.join(root, 'LICENSE'), 'Flecha license');
    const git = args => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
    git(['init', '-q']);
    git(['add', '.']);
    git(['-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'stage zero']);
    const sha = git(['rev-parse', 'HEAD']);
    const previous = 'a'.repeat(40);
    const output = path.join(dir, 'outputs');
    const log = path.join(dir, 'requests');
    const runtime = path.join(bin, 'fixture-node');
    await fs.writeFile(runtime, '#!/bin/sh\nexec node "$@"\n', { mode: 0o755 });
    await fs.writeFile(path.join(dir, 'LICENSE'), 'Node license');
    const preload = path.join(dir, 'runtime.mjs');
    await fs.writeFile(preload, `import fs from 'node:fs/promises';
      Object.defineProperty(process, 'execPath', { value: process.env.FIXTURE_NODE });
      if (process.env.FIXTURE_LOW_DISK) fs.statfs = async () => ({ bsize: 1, bavail: 0 });`);
    await fs.writeFile(path.join(bin, 'gh'), `#!/usr/bin/env node
      const fs = require('node:fs');
      const [op, endpoint] = process.argv.slice(2);
      if (op === 'api') {
        fs.appendFileSync(process.env.FIXTURE_LOG, endpoint + '\\n');
        if (endpoint.endsWith('/artifacts/7/zip')) fs.writeFileSync(1, fs.readFileSync(process.env.FIXTURE_ZIP));
        else if (endpoint.endsWith('/runs/1')) console.log(JSON.stringify({ id: 1, head_sha: '${previous}',
          head_repository: { full_name: 'owner/repo' }, head_branch: 'main', event: 'schedule', path: '.github/workflows/nightly-release.yml' }));
        else {
          const name = new URL('https://api.github.com/' + endpoint).searchParams.get('name');
          const hit = name && name === process.env.FIXTURE_ARTIFACT;
          console.log(JSON.stringify({ total_count: hit ? 1 : 0, artifacts: hit ? [{ id: 7, name, workflow_run: { id: 1 } }] : [] }));
        }
      } else throw Error('unexpected GitHub request');
    `, { mode: 0o755 });
    const stage = async (mode, artifact = '', extraEnv = {}) => {
      await fs.writeFile(output, '');
      const zip = path.join(dir, mode + '.zip');
      execFileSync(process.execPath, ['--import', preload, path.join(root, 'chromium/scripts/ci-release.mjs'), mode], {
        cwd: root, env: { ...process.env, GITHUB_ACTIONS: 'true', RUNNER_ENVIRONMENT: 'github-hosted', RUNNER_WORKSPACE: workspace,
          GITHUB_REPOSITORY: 'owner/repo', GITHUB_RUN_ID: '2', GITHUB_RUN_NUMBER: '2', GITHUB_OUTPUT: output,
          FLECHA_DEFAULT_BRANCH: 'main', FLECHA_REUSE_BUDGET_MINUTES: '60', FLECHA_STAGE_STARTED: String(Date.now()),
          ImageOS: 'ubuntu24', ImageVersion: 'fixture', FIXTURE_ARTIFACT: artifact, FIXTURE_NODE: runtime, FIXTURE_LOG: log,
          FIXTURE_ZIP: zip, PATH: bin + path.delimiter + process.env.PATH, ...extraEnv },
      });
      return fs.readFile(output, 'utf8');
    };
    assert.match(await stage('portable'), /^spent-ms=\d+\n$/);
    await assert.rejects(fs.stat(work), { code: 'ENOENT' });
    const [first] = (await fs.readFile(log, 'utf8')).split('\n');
    const artifact = new URL('https://api.github.com/' + first).searchParams.get('name');
    const [, key] = /^release-linux-x64-([a-f0-9]{64})-[a-f0-9]{64}-attempt-1$/.exec(artifact);
    const source = { sha: previous, chromium: '158', platform: 'linux-x64', nativeKey: key, depot: depotRevision };
    const tree = path.join(dir, 'tree');
    await fs.mkdir(path.join(tree, 'depot_tools'), { recursive: true });
    await fs.mkdir(path.join(tree, 'src/buildtools/linux64'), { recursive: true });
    await fs.writeFile(path.join(tree, 'src/buildtools/linux64/gn'), '#!/bin/sh\nexit 0\n', { mode: 0o755 });
    await fs.writeFile(path.join(tree, 'state.json'), JSON.stringify({ ...source, complete: false }));
    const checkpoint = checkpointPath(seed, 'native');
    saveCheckpoint(tree, checkpoint, false);
    execFileSync('7z', ['a', '-tzip', path.join(dir, 'prepare.zip'), path.basename(checkpoint)], { cwd: seed });
    await fs.rm(checkpoint);
    const realGit = execFileSync('sh', ['-c', 'command -v git'], { encoding: 'utf8' }).trim();
    await fs.writeFile(path.join(bin, 'git'), `#!/bin/sh
      case "$1:$2" in init:*/depot_tools|-C:*/depot_tools) exit 0;; esac
      exec "${realGit}" "$@"
    `, { mode: 0o755 });
    await fs.writeFile(path.join(bin, 'python3'), `#!/bin/sh
      case "$1:$2" in
        */ninja.py:*) kill -TERM "$PPID"; exit 0;;
        */gclient.py:sync) mkdir -p src/buildtools/linux64 src/chrome/app/theme/chromium && cp "$FIXTURE_GN" src/buildtools/linux64/gn;;
        */gclient.py:runhooks) exit 0;;
        *) exit 1;;
      esac
    `, { mode: 0o755 });
    assert.equal(await stage('prepare', 'native-linux-x64-' + key), 'compile=true\n');
    const restored = JSON.parse(await fs.readFile(path.join(work, 'state.json')));
    assert.equal(restored.sha, sha);
    assert.equal(restored.reusedFrom.archive, path.basename(checkpoint));
    await assert.rejects(stage('build'), error => error.status !== 0 && /Build cancelled/.test(error.stderr));
    await fs.rm(work, { recursive: true }); // A rerun starts on a fresh runner.
    assert.equal(await stage('prepare', 'native-linux-x64-' + key), 'compile=true\n');
    assert.deepEqual(JSON.parse(await fs.readFile(path.join(work, 'state.json'))), restored);
    assert.equal((await fs.readFile(log, 'utf8')).split('/artifacts/7/zip').length - 1, 2);
    await fs.rm(work, { recursive: true });

    // A warm tree without room for its measured checkpoint must yield to cold preparation.
    for (const subdir of ['patches', 'branding/mac', 'branding/logo']) {
      await fs.mkdir(path.join(root, 'chromium', subdir), { recursive: true });
    }
    await fs.writeFile(path.join(root, 'chromium/branding/BRANDING'), 'test');
    assert.equal(await stage('prepare', 'native-linux-x64-' + key, { FIXTURE_LOW_DISK: 'true',
      FIXTURE_GN: path.join(tree, 'src/buildtools/linux64/gn') }), 'compile=false\n');
    const handoff = await stage('checkpoint');
    assert.match(handoff, /^complete=false$/m);
    const handedOff = path.join(dir, 'cold-checkpoint');
    await fs.mkdir(handedOff);
    execFileSync('tar', ['-xf', path.join(root, /^archive=(.+)$/m.exec(handoff)[1]), '-C', handedOff]);
    const cold = JSON.parse(await fs.readFile(path.join(handedOff, 'state.json')));
    assert.equal(cold.sha, sha);
    assert.equal(cold.reusedFrom, undefined);
    await fs.rm(work, { recursive: true });
    await fs.writeFile(path.join(tree, 'chrome'), 'verified browser', { mode: 0o755 });
    const name = 'Flecha-linux-x64.tar.gz';
    execFileSync('tar', ['-czf', path.join(seed, name), '-C', tree, 'chrome']);
    await fs.writeFile(path.join(seed, 'linux-x64.json'), JSON.stringify({ ...source, args: 'original compiler flags' }));
    for (const file of [name, 'linux-x64.json']) {
      const digest = createHash('sha256').update(await fs.readFile(path.join(seed, file))).digest('hex');
      await fs.appendFile(path.join(seed, 'linux-x64-SHA256SUMS'), digest + '  ' + file + '\n');
    }
    execFileSync('7z', ['a', '-tzip', path.join(dir, 'portable.zip'), ...await fs.readdir(seed)], { cwd: seed });
    assert.equal(await stage('portable', artifact), 'package=' + artifact.replace(/-attempt-1$/, '') + '\n');
    const release = path.join(root, 'release');
    await verifyRelease(release, [{ platform: 'linux-x64' }], { ...source, sha });
    assert.equal(JSON.parse(await fs.readFile(path.join(release, 'linux-x64.json'))).reusedFrom.name, artifact);
    await assert.rejects(fs.stat(path.join(work, 'src')), { code: 'ENOENT' });
  });
