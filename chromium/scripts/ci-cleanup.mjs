// Only disposable GitHub-hosted VMs may remove preinstalled build tools.
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';

export async function cleanupXcodes(applications, selected, remove) {
  // Resolve aliases before deleting their targets, so discovery cannot break mid-cleanup.
  const names = (await fs.readdir(applications)).filter(name => /^Xcode.*\.app$/.test(name));
  const apps = await Promise.all(names.map(async name => {
    const app = path.join(applications, name);
    return { app, real: await fs.realpath(app) };
  }));
  for (const { app, real } of apps) {
    if (selected.startsWith(`${real}/`)) continue;
    await remove(app);
  }
}

async function main() {
  if (process.env.GITHUB_ACTIONS !== 'true' || process.env.RUNNER_ENVIRONMENT !== 'github-hosted') {
    throw new Error('Cleanup requires a disposable GitHub-hosted runner.');
  }

  const report = () => {
    if (process.platform === 'win32') {
      execFileSync('powershell.exe', ['-NoProfile', '-Command', 'Get-PSDrive -PSProvider FileSystem | Format-Table Name,Used,Free'], { stdio: 'inherit' });
    } else execFileSync('df', ['-h'], { stdio: 'inherit' });
  };
  report();
  if (process.platform === 'linux') {
    // Matches the successful portablelinux runner cleanup recorded in docs.
    execFileSync('sudo', ['rm', '-rf', '/usr/local/lib/android', '/usr/local/.ghcup',
      '/usr/lib/jvm', '/usr/lib/google-cloud-sdk', '/usr/share/dotnet', '/usr/share/swift'], { stdio: 'inherit' });
  } else if (process.platform === 'darwin') {
    const selected = await fs.realpath(execFileSync('xcode-select', ['-p'], { encoding: 'utf8' }).trim());
    await cleanupXcodes('/Applications', selected,
      app => execFileSync('sudo', ['rm', '-rf', app], { stdio: 'inherit' }));
    execFileSync('sudo', ['rm', '-rf', '/Users/runner/Library/Android/sdk'], { stdio: 'inherit' });
  } else if (process.platform === 'win32') {
    // Keep Visual Studio, Windows SDKs, Python and Node.
    for (const dir of ['C:\\Android', 'C:\\Program Files\\Java', 'C:\\Program Files\\dotnet']) {
      await fs.rm(dir, { recursive: true, force: true });
    }
  }
  report();
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
