import { createHash } from 'node:crypto';
import { execFile, execFileSync, spawn } from 'node:child_process';
import { promisify } from 'node:util';
import fs from 'node:fs/promises';
import path from 'node:path';

function preserveCancellation(error) {
  if (error?.code === 'REUSE_TERMINATION_FAILED') throw error;
  if (error?.code === 'ETIMEDOUT') return;
  if (error?.code === 'ABORT_ERR' || ['SIGINT', 'SIGTERM'].includes(error?.signal)) throw error;
}

const execFileAsync = promisify(execFile);

// Optional trees are disposable. Force-stop their process group at the owner deadline,
// then wait for inherited pipes to close before allowing cleanup or another attempt.
export async function runReuseCommand(command, args, { cwd, env, deadline, signal, capture = false } = {}) {
  signal?.throwIfAborted();
  const timeoutError = () => Object.assign(new Error('Native reuse reached its deadline.'), { code: 'ETIMEDOUT' });
  if (deadline !== undefined && deadline <= Date.now()) throw timeoutError();
  const windows = process.platform === 'win32';
  // Pipes and a new session keep sudo away from terminal/PTY process groups.
  const child = spawn(command, args, { cwd, env, detached: !windows, stdio: ['ignore', 'pipe', 'pipe'] });
  let stdout = '';
  if (capture) child.stdout.setEncoding('utf8').on('data', chunk => { stdout += chunk; });
  else child.stdout.pipe(process.stdout, { end: false });
  child.stderr.pipe(process.stderr, { end: false });
  let expired = false;
  let stopping;
  let terminationFailure;
  let rejectCompletion;
  const completion = new Promise((resolve, reject) => {
    rejectCompletion = reject;
    child.once('error', reject);
    child.once('close', (code, signal) => resolve({ code, signal }));
  });
  const killTree = async () => {
    if (!child.pid) return;
    const killEnv = withoutGitHubTokens(env || process.env);
    if (windows) {
      await execFileAsync('taskkill', ['/PID', String(child.pid), '/T', '/F'], { env: killEnv });
    } else if (command === 'sudo') {
      try {
        await execFileAsync('sudo', ['-n', 'kill', '-KILL', '--', String(-child.pid)], { env: killEnv });
      } catch (error) {
        try { process.kill(-child.pid, 0); } catch (probe) { if (probe.code === 'ESRCH') return; }
        throw error;
      }
    } else {
      try { process.kill(-child.pid, 'SIGKILL'); } catch (error) { if (error.code !== 'ESRCH') throw error; }
    }
  };
  const stop = () => {
    stopping ??= killTree().catch(cause => {
      // No fallback may modify the tree if stopping its previous owner failed.
      child.unref();
      child.stdout.destroy();
      child.stderr.destroy();
      terminationFailure = Object.assign(new Error('Cannot stop native reuse processes.', { cause }), { code: 'REUSE_TERMINATION_FAILED' });
      rejectCompletion(terminationFailure);
    });
  };
  signal?.addEventListener('abort', stop, { once: true });
  const timer = deadline === undefined ? undefined : setTimeout(() => { expired = true; stop(); }, deadline - Date.now());
  try {
    const result = await completion;
    await stopping;
    if (terminationFailure) throw terminationFailure;
    signal?.throwIfAborted();
    if (expired) throw timeoutError();
    if (result.code !== 0) throw Object.assign(new Error(`Native reuse command failed: ${result.code ?? result.signal}`), { signal: result.signal });
    return stdout;
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', stop);
  }
}

export function withoutGitHubTokens(environment) {
  const result = { ...environment };
  delete result.GH_TOKEN;
  delete result.GITHUB_TOKEN;
  return result;
}

export function reuseDeadline({ started, stageDeadline, budgetMinutes }) {
  const milliseconds = Number(budgetMinutes) * 60 * 1000;
  if (!/^\d+$/.test(String(budgetMinutes)) || !Number.isSafeInteger(milliseconds)) {
    throw new Error('Expected a cache-reuse budget in whole minutes.');
  }
  return Math.min(started + milliseconds, stageDeadline);
}

// Packaging and shell inputs are refreshed after restoration. Native inputs must match.
export function nativeKey(root, configuration) {
  const tree = execFileSync('git', ['ls-tree', '-r', '-z', 'HEAD', '--',
    'chromium/CHROMIUM_VERSION', 'chromium/patches', 'chromium/branding',
    'chromium/build', 'chromium/scripts/ci-windows-sdk.ps1'], { cwd: root, env: withoutGitHubTokens(process.env) });
  return createHash('sha256').update(tree).update(JSON.stringify(configuration)).digest('hex');
}

export function reusableArtifact(artifacts, platform, key) {
  const available = artifacts.filter(artifact => !artifact.expired);
  const native = available.find(artifact => artifact.name === `native-${platform}-${key}`);
  if (native) return { name: native.name, archive: 'native.tar.gz' };
  const prefix = `reuse-${platform}-${key}-`;
  const stages = available.filter(artifact => artifact.name.startsWith(prefix))
    .map(artifact => artifact.name.slice(prefix.length))
    .filter(stage => /^(0|[1-9]\d*)$/.test(stage))
    .sort((a, b) => Number(b) - Number(a));
  for (const stage of stages) {
    const name = `checkpoint-${platform}-${stage}`;
    if (available.some(artifact => artifact.name === name)) return { name, archive: 'checkpoint.tar.gz' };
  }
  return undefined;
}

export function trustedRun(run, { repository, branch, currentRun }) {
  return String(run.id) !== currentRun && run.head_repository?.full_name === repository &&
    run.path === '.github/workflows/nightly-release.yml' && run.head_branch === branch &&
    ['schedule', 'workflow_dispatch'].includes(run.event);
}

// Search only this repository's default-branch nightly workflow, including failed runs.
export async function* findReusableArtifacts({ repository, branch, currentRun, platform, key, deadline }, api) {
  const byRun = new Map();
  const runs = new Map();
  const excluded = new Set();
  let visited = 0;
  for (let page = 1; ; page++) {
    if (deadline !== undefined && Date.now() >= deadline) throw new Error('Native reuse reached the stage deadline.');
    // GitHub documents 100 as the maximum artifacts page size.
    const response = await api(`repos/${repository}/actions/artifacts?per_page=100&page=${page}`);
    visited += response.artifacts.length;
    for (const artifact of response.artifacts) {
      if (artifact.name !== `native-${platform}-${key}` &&
          !artifact.name.startsWith(`reuse-${platform}-${key}-`) &&
          !artifact.name.startsWith(`checkpoint-${platform}-`)) continue;
      const runId = String(artifact.workflow_run?.id);
      if (!artifact.workflow_run || artifact.expired || runId === currentRun) continue;
      const artifacts = byRun.get(runId) || [];
      artifacts.push(artifact);
      byRun.set(runId, artifacts);
      const candidate = reusableArtifact(artifacts.filter(item => !excluded.has(`${runId}/${item.name}`)), platform, key);
      if (!candidate) continue;
      if (!runs.has(runId)) runs.set(runId, await api(`repos/${repository}/actions/runs/${runId}`));
      const run = runs.get(runId);
      if (!trustedRun(run, { repository, branch, currentRun })) continue;
      excluded.add(`${runId}/${candidate.name}`);
      yield { ...candidate, run: runId, sha: run.head_sha };
    }
    if (visited >= response.total_count || !response.artifacts.length) return undefined;
  }
}

export async function downloadReusableTree(root, options, restore,
  gh = args => runReuseCommand('gh', args, { cwd: root, deadline: options.deadline, signal: options.signal, capture: true })) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(options.repository || '') || !options.branch || !/^\d+$/.test(options.currentRun || '')) {
    throw new Error('Missing GitHub context for native checkpoint reuse.');
  }
  const candidates = findReusableArtifacts(options, async endpoint => JSON.parse(await gh(['api', endpoint])));
  for (;;) {
    options.signal?.throwIfAborted();
    if (options.deadline !== undefined && Date.now() >= options.deadline) return undefined;
    let candidate;
    try {
      candidate = (await candidates.next()).value;
    } catch (error) {
      options.signal?.throwIfAborted();
      preserveCancellation(error);
      console.warn('Native checkpoint lookup unavailable; preparing a clean tree.');
      return undefined;
    }
    if (!candidate) return undefined;
    const directory = await fs.mkdtemp(path.join(root, 'native-download-'));
    let preserveDirectory = false;
    try {
      try {
        await gh(['run', 'download', candidate.run, '--repo', options.repository, '--name', candidate.name, '--dir', directory]);
      } catch (error) {
        preserveDirectory = error?.code === 'REUSE_TERMINATION_FAILED';
        options.signal?.throwIfAborted();
        preserveCancellation(error);
        console.warn('Native checkpoint download unavailable; checking other compatible trees.');
        continue;
      }
      // Refuse pre-existing trees. Only this successfully created directory may be removed.
      await fs.mkdir(options.work);
      try {
        await restore(path.join(directory, candidate.archive), candidate);
      } catch (error) {
        if (error?.code === 'REUSE_TERMINATION_FAILED') {
          preserveDirectory = true;
          throw error;
        }
        await fs.rm(options.work, { recursive: true });
        options.signal?.throwIfAborted();
        preserveCancellation(error);
        console.warn('Native checkpoint restoration failed; checking other compatible trees.');
        continue;
      }
    } finally {
      if (!preserveDirectory) await fs.rm(directory, { recursive: true, force: true });
    }
    console.log(`Restored native inputs from run ${candidate.run}, source ${candidate.sha}.`);
    return candidate;
  }
}
