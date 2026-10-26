// Share the compiled Linux browser while keeping each checkout's shell and profile separate.
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync, spawn } from 'node:child_process';

const root = fileURLToPath(new URL('../..', import.meta.url));
const args = process.argv.slice(2);
let out = process.env.FLECHA_CHROMIUM_OUT || path.join(os.homedir(), 'flecha-builds', 'chromium-158-linux', 'out', 'Dev');
let profile = path.join(root, '.flecha-profile');
let browserArgs = [];
const usage = 'npm run browser:linux -- [--out <absolute Chromium output directory>] [--profile <absolute user-data directory>] [-- <Chromium arguments>]';
for (let i = 0; i < args.length; i++) {
  if (args[i] === '--') { browserArgs = args.slice(i + 1); break; }
  if (args[i] === '--help') { console.log(usage); process.exit(0); }
  if (!['--out', '--profile'].includes(args[i]) || !args[i + 1] || args[i + 1].startsWith('--')) throw new Error(usage);
  if (args[i] === '--out') out = args[++i];
  else profile = args[++i];
}
if (process.platform !== 'linux') throw new Error('This launcher requires a Linux Chromium build.');
if (!path.isAbsolute(out) || !path.isAbsolute(profile)) throw new Error('The output and profile paths must be absolute.');
if (browserArgs.some(arg => /^--(?:user-data-dir|flecha-shell-dir)(?:=|$)/.test(arg))) {
  throw new Error('Use --profile for browser data. The shell directory always belongs to this checkout.');
}
const binary = path.join(out, 'chrome');
if (!(await fs.stat(binary)).isFile()) throw new Error(`No Chromium executable at ${binary}`);
try {
  await fs.lstat(path.join(profile, 'SingletonLock'));
  throw new Error(`The profile is locked: ${profile}. Quit its browser normally before launching.`);
} catch (error) {
  if (error.code !== 'ENOENT') throw error;
}
const env = { ...process.env };
delete env.__PYVENV_LAUNCHER__;
for (const script of ['chromium/shell/build.mjs', 'chromium/native-host/build.mjs']) {
  execFileSync(process.execPath, [path.join(root, script)], { cwd: root, env, stdio: 'inherit' });
}
execFileSync(process.execPath, [path.join(root, 'chromium/scripts/install-native-host.mjs'), '--profile', profile], { cwd: root, env, stdio: 'inherit' });
const browser = spawn(binary, [
  `--user-data-dir=${profile}`,
  `--flecha-shell-dir=${env.FLECHA_SHELL_OUT || path.join(root, 'chromium/shell/dist')}`,
  '--no-first-run', '--no-default-browser-check',
  ...browserArgs,
], { cwd: root, env, stdio: 'inherit' });
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => browser.kill(signal));
browser.once('error', error => { console.error(error.message); process.exitCode = 1; });
browser.once('exit', code => { process.exitCode = code ?? 1; });
