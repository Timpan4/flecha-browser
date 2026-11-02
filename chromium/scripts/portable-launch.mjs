// A portable nightly keeps its profile and bundled native host beside the browser.
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { spawn, execFileSync } from 'node:child_process';

const here = fileURLToPath(new URL('.', import.meta.url));
const windows = process.platform === 'win32';
const profile = path.join(here, 'profile');
const host = path.join(here, 'native-host');
const name = 'dev.oprimo.flecha.host';
const args = process.argv.slice(2);
if (args.some(arg => /^--(?:user-data-dir|flecha-shell-dir)(?:=|$)/.test(arg))) {
  throw new Error('The portable launcher manages its own profile and shell directory.');
}
await fs.mkdir(path.join(profile, 'NativeMessagingHosts'), { recursive: true });
const launcher = path.join(host, windows ? 'flecha-host.cmd' : 'flecha-host');
const quote = text => `'${text.replaceAll("'", "'\\''")}'`;
if (windows && /[%\r\n]/.test(here)) throw new Error('Move the portable browser to a path without percent signs or line breaks.');
await fs.writeFile(launcher, windows
  ? '@echo off\r\n"%~dp0..\\node.exe" "%~dp0flecha-host.cjs" %*\r\n'
  : `#!/bin/sh\nexec ${quote(path.join(here, 'node'))} ${quote(path.join(host, 'flecha-host.cjs'))} "$@"\n`, { mode: 0o755 });
const manifest = path.join(profile, 'NativeMessagingHosts', `${name}.json`);
await fs.writeFile(manifest, JSON.stringify({ name, description: 'Flecha nightly native host', path: launcher,
  type: 'stdio', allowed_origins: ['chrome-extension://abdiaepfgbbhdjaecjipmaailijaaide/'] }, null, 2));
if (windows) execFileSync('reg.exe', ['ADD', `HKCU\\Software\\Chromium\\NativeMessagingHosts\\${name}`, '/ve', '/t', 'REG_SZ', '/d', manifest, '/f'], { stdio: 'inherit' });
const browser = spawn(path.join(here, windows ? 'chrome.exe' : 'chrome'), [
  `--user-data-dir=${profile}`, `--flecha-shell-dir=${path.join(here, 'resources/flecha-shell')}`, ...args,
], { stdio: 'inherit' });
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => browser.kill(signal));
browser.once('error', error => { console.error(error.message); process.exitCode = 1; });
browser.once('exit', code => { process.exitCode = code ?? 1; });
