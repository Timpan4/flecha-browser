// Hosted release stages keep one prepared Chromium tree at a stable absolute path.
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn, execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { pipeline } from 'node:stream/promises';
import { createReadStream } from 'node:fs';
import { nativeKey, downloadReusableTree, withoutGitHubTokens } from './ci-reuse.mjs';

const root = fileURLToPath(new URL('../..', import.meta.url));
const windows = process.platform === 'win32';
const mac = process.platform === 'darwin';
const cpu = mac ? 'arm64' : 'x64';
const platform = `${mac ? 'macos' : windows ? 'windows' : 'linux'}-${cpu}`;
const python = windows ? path.join(process.env.pythonLocation || '', 'python.exe') : 'python3';
export const depotRevision = 'a20039648c0de69d8156906b18052bd47e9bf230';
// Node sets the native working directory; GNU tar receives slash-separated archive paths.
const checkpointTar = windows ? path.join(process.env.ProgramFiles, 'Git/usr/bin/tar.exe') : 'tar';

function releaseArgs(template, { platform, version, buildNumber }) {
  let args = template.replace(/^target_cpu = .*$/m, `target_cpu = "${platform === 'macos-arm64' ? 'arm64' : 'x64'}"`);
  // Nightlies must never consult the stable upstream Sparkle feed.
  args = args.replace(/^flecha_sparkle_.*$/gm, '');
  if (!platform.startsWith('macos-')) args = args.replace(/^flecha_shell_dir = .*$/m, '');
  return `${args}\nuse_siso = false\nflecha_version = "${version}"\nflecha_build_number = "${buildNumber}"\n`;
}

// Keep profile checkout aligned with the GN arguments written for this platform.
export async function releaseConfiguration(options) {
  const template = options.platform === 'linux-x64' ? 'linux-release.gn' : 'release.gn';
  const args = releaseArgs(await fs.readFile(path.join(root, 'chromium/build', template), 'utf8'), options);
  const checkoutPgoProfiles = /^\s*chrome_pgo_phase\s*=\s*2\b/m.test(args);
  const gclient = `solutions = [{"name": "src", "url": "https://chromium.googlesource.com/chromium/src.git", "managed": False, "custom_deps": {}, "custom_vars": {"checkout_configuration": "small", "checkout_pgo_profiles": ${checkoutPgoProfiles ? 'True' : 'False'}}}]\n`;
  return { args, gclient };
}

export function checkpointIdentity({ sha, chromium, platform, depot }) {
  return { sha, chromium, platform, depot };
}

export function checkIdentity(actual, expected) {
  for (const key of Object.keys(expected)) {
    if (actual[key] !== expected[key]) throw new Error(`Checkpoint ${key} does not match this build.`);
  }
  if (typeof actual.complete !== 'boolean') throw new Error('Checkpoint has no completion state.');
}

export async function prepareNativeTree({ work, depot, src, configuration, run, chromium }) {
  await fs.mkdir(work);
  await fs.mkdir(depot);
  run('git', ['init', depot], work);
  run('git', ['-C', depot, 'fetch', '--depth=1', 'https://chromium.googlesource.com/chromium/tools/depot_tools.git', depotRevision], work);
  run('git', ['-C', depot, 'checkout', '--detach', 'FETCH_HEAD'], work);
  await fs.writeFile(path.join(work, '.gclient'), configuration.gclient);
  run(python, [path.join(depot, 'gclient.py'), 'sync', '--no-history', '--nohooks', '--revision', `src@${chromium}`], work);
  if (process.platform === 'linux') run('sudo', ['bash', 'build/install-build-deps.sh', '--no-prompt']);
  run(python, [path.join(depot, 'gclient.py'), 'runhooks'], work);
  for (const patch of (await fs.readdir(path.join(root, 'chromium/patches'))).filter(p => p.endsWith('.patch')).sort()) {
    run('git', ['apply', path.join(root, 'chromium/patches', patch)]);
  }
  const theme = path.join(src, 'chrome/app/theme/chromium');
  await fs.copyFile(path.join(root, 'chromium/branding/BRANDING'), path.join(theme, 'BRANDING'));
  await fs.cp(path.join(root, 'chromium/branding/mac'), path.join(theme, 'mac'), { recursive: true });
  for (const png of (await fs.readdir(path.join(root, 'chromium/branding/logo'))).filter(p => p.endsWith('.png'))) {
    await fs.copyFile(path.join(root, 'chromium/branding/logo', png), path.join(theme, png));
  }
}

export async function validateReusableTree(work, expected) {
  checkIdentity(JSON.parse(await fs.readFile(path.join(work, 'state.json'), 'utf8')), expected);
  if (!(await fs.stat(path.join(work, 'src'))).isDirectory() || !(await fs.stat(path.join(work, 'depot_tools'))).isDirectory()) {
    throw new Error('Reusable checkpoint has no source tree.');
  }
}

export async function configureReleaseTree({ root, work, identity, key, reused, args, generate }) {
  const src = path.join(work, 'src');
  const out = path.join(src, 'out/Release');
  const stateFile = path.join(work, 'state.json');
  // The restored extension can contain files removed from the current shell build.
  await fs.rm(path.join(src, 'flecha/shell'), { recursive: true, force: true });
  await fs.mkdir(path.join(src, 'flecha'), { recursive: true });
  await fs.cp(path.join(root, 'chromium/shell/dist'), path.join(src, 'flecha/shell'), { recursive: true });
  await fs.mkdir(out, { recursive: true });
  await fs.writeFile(path.join(out, 'args.gn'), args);
  generate();
  // A compatible native tree still needs current shell packaging and provenance.
  await fs.writeFile(stateFile, JSON.stringify({ ...identity, nativeKey: key, complete: false,
    ...(reused ? { reusedFrom: reused } : {}) }));
}

export function saveCheckpoint(work, archive, complete) {
  // Windows' system tar crashes on the Chromium tree. Git ships GNU tar.
  execFileSync(checkpointTar, [...(windows ? ['--force-local'] : []), '-czf', archive.replaceAll(path.sep, '/'),
    ...(complete ? ['state.json'] : ['.'])], { cwd: work, env: withoutGitHubTokens(process.env), stdio: 'inherit' });
}

export async function preserveNativeOutputs(work, archive, save = saveCheckpoint) {
  let owned = false;
  try {
    await fs.writeFile(archive, '', { flag: 'wx' });
    owned = true;
    save(work, archive, false);
    return true;
  } catch (error) {
    if (owned) await fs.rm(archive, { force: true });
    if (error?.code === 'ABORT_ERR' || ['SIGINT', 'SIGTERM'].includes(error?.signal)) throw error;
    console.warn('Native snapshot unavailable; continuing with release packaging.');
    return false;
  }
}

export async function restoreCheckpoint(work, archive, { create = true } = {}) {
  if (create) await fs.mkdir(work);
  // MSYS otherwise copies link targets, which fails for dangling or later-extracted targets.
  const env = { ...withoutGitHubTokens(process.env), ...(windows ? { MSYS: 'winsymlinks:nativestrict' } : {}) };
  execFileSync(checkpointTar, [...(windows ? ['--force-local'] : []), '-xzf', archive.replaceAll(path.sep, '/')], { cwd: work, env, stdio: 'inherit' });
  await fs.rm(archive);
}

export async function compile(command, args, { cwd, env, milliseconds, signal }) {
  signal?.throwIfAborted();
  const child = spawn(command, args, { cwd, env, stdio: 'inherit', detached: !windows });
  let deadline = false;
  const stop = () => {
    if (!child.pid) return;
    if (windows) execFileSync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { stdio: 'inherit' });
    else process.kill(-child.pid, 'SIGINT');
  };
  const abort = () => stop();
  signal?.addEventListener('abort', abort, { once: true });
  const timer = milliseconds === undefined ? undefined : setTimeout(() => { deadline = true; stop(); }, milliseconds);
  try {
    const code = await new Promise((resolve, reject) => {
      child.once('error', reject);
      child.once('exit', (code, signal) => resolve({ code, signal }));
    });
    signal?.throwIfAborted();
    if (deadline) return false;
    if (code.code !== 0) throw new Error(`Chromium build failed: ${code.code ?? code.signal}`);
    return true;
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', abort);
  }
}

async function main() {
  if (process.env.GITHUB_ACTIONS !== 'true' || process.env.RUNNER_ENVIRONMENT !== 'github-hosted') {
    throw new Error('This script writes only to disposable GitHub-hosted build trees.');
  }
  const mode = process.argv[2];
  if (!['prepare', 'restore', 'build', 'checkpoint'].includes(mode)) throw new Error('Expected prepare, restore, build or checkpoint.');
  const work = path.resolve(process.env.RUNNER_WORKSPACE, 'flecha-chromium');
  if (mode === 'restore') {
    await restoreCheckpoint(work, path.join(root, 'checkpoint/checkpoint.tar.gz'));
    return;
  }
  const src = path.join(work, 'src');
  const depot = path.join(work, 'depot_tools');
  const out = path.join(src, 'out/Release');
  const env = { ...withoutGitHubTokens(process.env), DEPOT_TOOLS_UPDATE: '0', DEPOT_TOOLS_WIN_TOOLCHAIN: '0',
    ...(windows ? { GIT_CONFIG_COUNT: '1', GIT_CONFIG_KEY_0: 'core.longpaths', GIT_CONFIG_VALUE_0: 'true' } : {}),
    PATH: `${depot}${path.delimiter}${process.env.PATH}` };
  const run = (command, args, cwd = src) => execFileSync(command, args, { cwd, env, stdio: 'inherit' });
  const capture = (command, args, cwd = src) => execFileSync(command, args, { cwd, env, encoding: 'utf8' }).trim();
  const chromium = (await fs.readFile(path.join(root, 'chromium/CHROMIUM_VERSION'), 'utf8')).trim();
  const sha = capture('git', ['rev-parse', 'HEAD'], root);
  const identity = checkpointIdentity({ sha, chromium, platform, depot: depotRevision });
  const stateFile = path.join(work, 'state.json');
  const gn = path.join(src, 'buildtools', windows ? 'win/gn.exe' : mac ? 'mac/gn' : 'linux64/gn');
  const archive = path.join(root, 'checkpoint.tar.gz');
  const showDisk = () => {
    const disk = capture(windows ? 'fsutil' : 'df', windows ? ['volume', 'diskfree', path.parse(work).root] : ['-h', work], work);
    console.log(disk);
  };
  const output = async (key, value) => {
    if (process.env.GITHUB_OUTPUT) await fs.appendFile(process.env.GITHUB_OUTPUT, `${key}=${value}\n`);
  };

  if (mode === 'prepare') {
    const { version } = JSON.parse(await fs.readFile(path.join(root, 'package.json'), 'utf8'));
    const configuration = await releaseConfiguration({ platform, version, buildNumber: process.env.GITHUB_RUN_NUMBER });
    const normalized = await releaseConfiguration({ platform, version, buildNumber: '0' });
    const image = { os: process.env.ImageOS, version: process.env.ImageVersion };
    const key = nativeKey(root, { ...normalized, platform, depot: depotRevision, work,
      preparation: prepareNativeTree.toString(),
      image,
      // Unknown runner images must not share native outputs across runs.
      ...(!image.os || !image.version ? { uncachedRun: process.env.GITHUB_RUN_ID } : {}),
      xcode: mac ? capture('xcodebuild', ['-version'], root) : null,
      visualStudio: windows ? capture(path.join(process.env['ProgramFiles(x86)'], 'Microsoft Visual Studio/Installer/vswhere.exe'),
        ['-latest', '-products', '*', '-requires', 'Microsoft.VisualStudio.Component.VC.Tools.x86.x64', '-property', 'installationVersion'], root) : null });
    const reused = await downloadReusableTree(root, { repository: process.env.GITHUB_REPOSITORY,
      branch: process.env.FLECHA_DEFAULT_BRANCH, currentRun: process.env.GITHUB_RUN_ID, platform, key, work },
    async (archive, candidate) => {
      await restoreCheckpoint(work, archive, { create: false });
      await validateReusableTree(work, { ...identity, sha: candidate.sha, nativeKey: key });
    });
    if (!reused) {
      await prepareNativeTree({ work, depot, src, configuration, run, chromium });
    }
    await configureReleaseTree({ root, work, identity, key, reused, args: configuration.args,
      generate: () => {
        if (reused && process.platform === 'linux') run('sudo', ['bash', 'build/install-build-deps.sh', '--no-prompt']);
        run(gn, ['gen', 'out/Release', '--fail-on-unused-args']);
      } });
    showDisk();
    return;
  }

  const state = JSON.parse(await fs.readFile(stateFile, 'utf8'));
  checkIdentity(state, identity);
  if (mode === 'checkpoint') {
    // Completed stages carry only a marker; browser packages have a separate artifact.
    saveCheckpoint(work, archive, state.complete);
    if (!state.complete && state.nativeKey) {
      await fs.writeFile(path.join(root, 'reuse.json'), JSON.stringify(state));
      await output('reuse-key', state.nativeKey);
    }
    showDisk();
    return;
  }
  if (state.complete) return;
  if (process.platform === 'linux') run('sudo', ['bash', 'build/install-build-deps.sh', '--no-prompt']);
  showDisk();
  // The successful portablelinux workflow uses five-hour compilation stages.
  // Include restoration/setup in that allowance, preserving its observed handoff margin.
  const started = Number(process.env.FLECHA_STAGE_STARTED);
  if (!Number.isFinite(started) || started <= 0) throw new Error('Missing stage start time.');
  const remaining = 5 * 60 * 60 * 1000 - (Date.now() - started);
  if (remaining <= 0) throw new Error('Restoration used the compilation allowance; checkpoint remains available.');
  const controller = new AbortController();
  const cancel = () => controller.abort(new Error('Build cancelled.'));
  process.once('SIGINT', cancel);
  process.once('SIGTERM', cancel);
  let complete;
  try {
    complete = await compile(python, [path.join(depot, 'ninja.py'), '-C', 'out/Release', 'chrome'],
      { cwd: src, env, milliseconds: remaining, signal: controller.signal });
  } finally {
    process.removeListener('SIGINT', cancel);
    process.removeListener('SIGTERM', cancel);
  }
  if (!complete) {
    console.log('Compilation checkpoint requested; next stage will resume.');
    return;
  }

  // Save compiled outputs before packaging, so packaging failures remain recoverable.
  if (state.nativeKey && await preserveNativeOutputs(work, path.join(root, 'native.tar.gz'))) {
    await output('native-key', state.nativeKey);
  }

  const dist = path.join(root, 'release');
  await fs.mkdir(dist, { recursive: true });
  const provenance = { ...identity, nativeKey: state.nativeKey, reusedFrom: state.reusedFrom,
    args: await fs.readFile(path.join(out, 'args.gn'), 'utf8') };
  await fs.writeFile(path.join(dist, `${platform}.json`), JSON.stringify(provenance, null, 2) + '\n');
  if (mac) {
    // Existing signing code seals the app ad hoc. No stable updater or notarization credentials.
    run('bash', [path.join(root, 'chromium/scripts/package-release.sh')], root);
    run('tar', ['-czf', path.join(dist, 'Flecha-macos-arm64.tar.gz'), '-C', path.join(out, 'flecha-release/Flecha'), 'Flecha.app'], root);
  } else {
    const stage = path.join(root, 'portable');
    await fs.mkdir(stage);
    const deps = capture(gn, ['desc', 'out/Release', '//chrome:chrome', 'runtime_deps']).split(/\r?\n/);
    for (const file of deps) {
      if (!file) continue;
      const source = path.resolve(out, file);
      const relative = path.relative(out, source);
      if (relative.startsWith('..') || path.isAbsolute(relative)) throw new Error(`Runtime dependency outside output: ${file}`);
      const target = path.join(stage, relative);
      await fs.mkdir(path.dirname(target), { recursive: true });
      await fs.cp(source, target, { recursive: true, preserveTimestamps: true });
    }
    await fs.cp(path.join(root, 'chromium/shell/dist'), path.join(stage, 'resources/flecha-shell'), { recursive: true });
    await fs.mkdir(path.join(stage, 'native-host'), { recursive: true });
    await fs.copyFile(process.execPath, path.join(stage, windows ? 'node.exe' : 'node'));
    await fs.copyFile(path.join(root, 'chromium/native-host/dist/flecha-host.cjs'), path.join(stage, 'native-host/flecha-host.cjs'));
    await fs.copyFile(path.join(root, 'chromium/scripts/portable-launch.mjs'), path.join(stage, 'portable-launch.mjs'));
    await fs.copyFile(path.join(src, 'LICENSE'), path.join(stage, 'CHROMIUM-LICENSE'));
    await fs.copyFile(path.join(root, 'LICENSE'), path.join(stage, 'FLECHA-LICENSE'));
    const nodeLicense = path.join(path.dirname(process.execPath), windows ? 'LICENSE' : '../LICENSE');
    await fs.copyFile(nodeLicense, path.join(stage, 'NODE-LICENSE'));
    if (windows) {
      await fs.writeFile(path.join(stage, 'Flecha.cmd'), '@echo off\r\n"%~dp0node.exe" "%~dp0portable-launch.mjs" %*\r\n');
      run('7z', ['a', '-tzip', path.join(dist, 'Flecha-windows-x64.zip'), '.'], stage);
    } else {
      await fs.chmod(path.join(stage, 'node'), 0o755);
      await fs.writeFile(path.join(stage, 'flecha'), '#!/bin/sh\nhere="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"\nexec "$here/node" "$here/portable-launch.mjs" "$@"\n', { mode: 0o755 });
      run('tar', ['-czf', path.join(dist, 'Flecha-linux-x64.tar.gz'), '-C', stage, '.'], root);
    }
  }
  for (const file of await fs.readdir(dist)) {
    const hash = createHash('sha256');
    await pipeline(createReadStream(path.join(dist, file)), hash);
    await fs.appendFile(path.join(dist, `${platform}-SHA256SUMS`), `${hash.digest('hex')}  ${file}\n`);
  }
  // Publish completion only after every packaging and checksum operation succeeds.
  await fs.writeFile(stateFile, JSON.stringify({ ...state, complete: true }));
  showDisk();
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
