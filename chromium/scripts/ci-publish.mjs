// Only a platform's complete, checksummed package from this source commit can be released.
import fs from 'node:fs/promises';
import { createReadStream } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { pipeline } from 'node:stream/promises';
import { execFileSync } from 'node:child_process';

export async function verifyRelease(dir, platforms, { sha, chromium }) {
  const files = new Set(await fs.readdir(dir));
  for (const { platform } of platforms) {
    const archive = `Flecha-${platform}.${platform.startsWith('windows-') ? 'zip' : 'tar.gz'}`;
    const provenanceName = `${platform}.json`;
    const checksumsName = `${platform}-SHA256SUMS`;
    for (const file of [archive, provenanceName, checksumsName]) {
      if (!files.has(file)) throw new Error(`Missing release asset: ${file}`);
      files.delete(file);
    }
    const provenance = JSON.parse(await fs.readFile(path.join(dir, provenanceName), 'utf8'));
    if (provenance.sha !== sha || provenance.chromium !== chromium || provenance.platform !== platform) {
      throw new Error(`Release provenance mismatch: ${platform}`);
    }
    const checksums = new Map();
    for (const line of (await fs.readFile(path.join(dir, checksumsName), 'utf8')).trim().split('\n')) {
      const match = /^([a-f0-9]{64})  ([^/\\]+)$/.exec(line);
      if (!match || checksums.has(match[2])) throw new Error(`Invalid checksum manifest: ${platform}`);
      checksums.set(match[2], match[1]);
    }
    if (checksums.size !== 2 || !checksums.has(archive) || !checksums.has(provenanceName)) {
      throw new Error(`Checksum manifest does not cover the package and provenance: ${platform}`);
    }
    for (const [file, expected] of checksums) {
      const hash = createHash('sha256');
      await pipeline(createReadStream(path.join(dir, file)), hash);
      if (hash.digest('hex') !== expected) throw new Error(`Checksum mismatch: ${file}`);
    }
  }
  if (files.size) throw new Error(`Unexpected release assets: ${[...files].join(', ')}`);
}

// Platforms publish separately. The first to finish creates the run's release
// without assets, a single API call, so a concurrent platform's create fails
// instead of leaving a second draft. Every platform then uploads its own assets.
// The release is public during uploads, so the platform's SHA256SUMS file goes
// last and marks it complete. An interrupted upload leaves no checksum file, and
// a retry removes it before replacing the platform's other files.
export function publishAssets({ repository, tag, sha, title, notes, files, checksums }, gh) {
  let createError;
  try {
    gh(['release', 'create', tag, '--repo', repository, '--target', sha, '--title', title,
      '--prerelease', '--latest=false', '--notes-file', notes]);
  } catch (error) {
    // Another platform, or an earlier attempt of this job, may have created it first.
    createError = error;
  }
  let release;
  try {
    release = JSON.parse(gh(['release', 'view', tag, '--repo', repository, '--json', 'assets']));
  } catch (error) {
    throw createError ?? error;
  }
  const { object } = JSON.parse(gh(['api', `repos/${repository}/git/ref/tags/${tag}`]));
  if (object?.type !== 'commit' || object.sha !== sha) throw new Error(`Release tag ${tag} does not point to ${sha}.`);
  const marker = path.basename(checksums);
  if (release.assets.some(({ name }) => name === marker)) {
    gh(['release', 'delete-asset', tag, marker, '--repo', repository, '--yes']);
  }
  gh(['release', 'upload', tag, ...files, '--repo', repository, '--clobber']);
  try {
    gh(['release', 'upload', tag, checksums, '--repo', repository]);
  } catch (error) {
    // A failed upload can leave an empty asset in GitHub's 'starter' state under this name.
    // Remove it so it cannot pass for a finished platform; the original failure is what matters.
    try { gh(['release', 'delete-asset', tag, marker, '--repo', repository, '--yes']); } catch {}
    throw error;
  }
}

async function main() {
  const root = fileURLToPath(new URL('../..', import.meta.url));
  const platforms = JSON.parse(await fs.readFile(path.join(root, 'chromium/build/ci-platforms.json'), 'utf8'));
  const selected = platforms.filter(({ platform }) => platform === process.env.FLECHA_PLATFORM);
  if (selected.length !== 1) throw new Error(`Unknown release platform: ${process.env.FLECHA_PLATFORM}`);
  const chromium = (await fs.readFile(path.join(root, 'chromium/CHROMIUM_VERSION'), 'utf8')).trim();
  const { GITHUB_SHA: sha, GITHUB_RUN_ID: run, GITHUB_REPOSITORY: repository, GITHUB_SERVER_URL: server } = process.env;
  if (!sha || !/^\d+$/.test(run || '') || !repository || !server) throw new Error('Missing GitHub release context.');
  const dir = path.join(root, 'release');
  await verifyRelease(dir, selected, { sha, chromium });
  const tag = `nightly-${run}`;
  const notes = path.join(root, 'release-notes.md');
  await fs.writeFile(notes, `Nightly build of Flecha from ${sha}, based on Chromium ${chromium}.\n\n` +
    `Build: ${server}/${repository}/actions/runs/${run}\n\n` +
    'Each platform is added when its own build finishes, so a platform can be missing while it still builds or after its build fails. Each platform uploads its SHA256SUMS file last, so a platform is complete only when that file downloads and its checksums verify.\n\n' +
    'Linux and Windows are portable packages. Extract them to a writable directory and run flecha or Flecha.cmd. Their profiles stay beside the browser. Windows registers the bundled native host for your user.\n\n' +
    'The macOS app is ad hoc signed and is not notarized. Its native host can be registered with chromium/scripts/install-native-host.sh --app /path/to/Flecha.app from the matching source checkout.\n\n' +
    'These prereleases do not use the stable Sparkle update feed. SHA256SUMS files cover each package and its source provenance.\n');
  // verifyRelease left exactly this platform's archive, provenance and checksum file in dir.
  const checksums = path.join(dir, `${selected[0].platform}-SHA256SUMS`);
  const files = (await fs.readdir(dir)).map(file => path.join(dir, file)).filter(file => file !== checksums);
  publishAssets({ repository, tag, sha, notes, files, checksums, title: `Nightly ${new Date().toISOString().slice(0, 10)} (${run})` },
    args => execFileSync('gh', args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'inherit'] }));
  await fs.appendFile(process.env.GITHUB_STEP_SUMMARY,
    `Published ${selected[0].platform} to [${tag}](${server}/${repository}/releases/tag/${tag}) from ${sha}.\n`);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
