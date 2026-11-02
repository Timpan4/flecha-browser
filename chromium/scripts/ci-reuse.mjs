import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs/promises';
import path from 'node:path';

function preserveCancellation(error) {
  if (error?.code === 'ETIMEDOUT') return;
  if (error?.code === 'ABORT_ERR' || ['SIGINT', 'SIGTERM'].includes(error?.signal)) throw error;
}

export function withoutGitHubTokens(environment) {
  const result = { ...environment };
  delete result.GH_TOKEN;
  delete result.GITHUB_TOKEN;
  return result;
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
export function findReusableArtifact({ repository, branch, currentRun, platform, key, deadline }, api, excluded = new Set()) {
  const byRun = new Map();
  const rejectedRuns = new Set();
  let visited = 0;
  for (let page = 1; ; page++) {
    if (deadline !== undefined && Date.now() >= deadline) throw new Error('Native reuse reached the stage deadline.');
    // GitHub documents 100 as the maximum artifacts page size.
    const response = api(`repos/${repository}/actions/artifacts?per_page=100&page=${page}`);
    visited += response.artifacts.length;
    for (const artifact of response.artifacts) {
      if (artifact.name !== `native-${platform}-${key}` &&
          !artifact.name.startsWith(`reuse-${platform}-${key}-`) &&
          !artifact.name.startsWith(`checkpoint-${platform}-`)) continue;
      const runId = String(artifact.workflow_run?.id);
      if (!artifact.workflow_run || artifact.expired || runId === currentRun || rejectedRuns.has(runId)) continue;
      const artifacts = byRun.get(runId) || [];
      artifacts.push(artifact);
      byRun.set(runId, artifacts);
      const candidate = reusableArtifact(artifacts.filter(item => !excluded.has(`${runId}/${item.name}`)), platform, key);
      if (!candidate) continue;
      const run = api(`repos/${repository}/actions/runs/${runId}`);
      if (trustedRun(run, { repository, branch, currentRun })) return { ...candidate, run: runId, sha: run.head_sha };
      rejectedRuns.add(runId);
    }
    if (visited >= response.total_count || !response.artifacts.length) return undefined;
  }
}

export async function downloadReusableTree(root, options, restore,
  gh = args => {
    const timeout = options.deadline === undefined ? undefined : options.deadline - Date.now();
    if (timeout !== undefined && timeout <= 0) throw new Error('Native reuse reached the stage deadline.');
    return execFileSync('gh', args, { cwd: root, encoding: 'utf8', timeout });
  }) {
  if (!/^[\w.-]+\/[\w.-]+$/.test(options.repository || '') || !options.branch || !/^\d+$/.test(options.currentRun || '')) {
    throw new Error('Missing GitHub context for native checkpoint reuse.');
  }
  const excluded = new Set();
  for (;;) {
    let candidate;
    try {
      candidate = findReusableArtifact(options, endpoint => JSON.parse(gh(['api', endpoint])), excluded);
    } catch (error) {
      preserveCancellation(error);
      console.warn('Native checkpoint lookup unavailable; preparing a clean tree.');
      return undefined;
    }
    if (!candidate) return undefined;
    excluded.add(`${candidate.run}/${candidate.name}`);
    const directory = await fs.mkdtemp(path.join(root, 'native-download-'));
    try {
      try {
        await gh(['run', 'download', candidate.run, '--repo', options.repository, '--name', candidate.name, '--dir', directory]);
      } catch (error) {
        preserveCancellation(error);
        console.warn('Native checkpoint download unavailable; checking other compatible trees.');
        continue;
      }
      // Refuse pre-existing trees. Only this successfully created directory may be removed.
      await fs.mkdir(options.work);
      try {
        await restore(path.join(directory, candidate.archive), candidate);
      } catch (error) {
        await fs.rm(options.work, { recursive: true });
        preserveCancellation(error);
        console.warn('Native checkpoint restoration failed; checking other compatible trees.');
        continue;
      }
    } finally {
      await fs.rm(directory, { recursive: true, force: true });
    }
    console.log(`Restored native inputs from run ${candidate.run}, source ${candidate.sha}.`);
    return candidate;
  }
}
