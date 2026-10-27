// Register the development import helper for an explicit Flecha user-data directory.
// Windows Chromium also requires a per-user registry entry. No elevation is needed.
import fs from 'node:fs/promises';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync } from 'node:child_process';
import { createRequire } from 'node:module';

const args = process.argv.slice(2);
if (args.length !== 2 || args[0] !== '--profile' || !path.isAbsolute(args[1])) {
  throw new Error('Usage: node chromium/scripts/install-native-host.mjs --profile <absolute user-data-dir>. On Windows this also registers the helper in HKCU.');
}
createRequire(import.meta.url)('node:sqlite');
const profile = args[1];
const hostDir = path.join(profile, 'native-host');
const manifests = path.join(profile, 'NativeMessagingHosts');
const name = 'dev.oprimo.flecha.host';
const source = fileURLToPath(new URL('../native-host/dist/flecha-host.cjs', import.meta.url));
await fs.mkdir(hostDir, { recursive: true });
await fs.mkdir(manifests, { recursive: true });
const script = path.join(hostDir, 'flecha-host.cjs');
await fs.copyFile(source, script);
const windows = process.platform === 'win32';
const launcher = path.join(hostDir, windows ? 'flecha-host.cmd' : 'flecha-host');
const quote = (s) => `'${s.replaceAll("'", "'\\''")}'`;
const batchQuote = (s) => `"${s.replaceAll('%', '%%')}"`;
await fs.writeFile(launcher, windows
  ? `@echo off\r\n${batchQuote(process.execPath)} ${batchQuote(script)} %*\r\n`
  : `#!/bin/sh\nexec ${quote(process.execPath)} ${quote(script)} "$@"\n`, { mode: 0o755 });
const manifest = path.join(manifests, `${name}.json`);
await fs.writeFile(manifest, JSON.stringify({ name, description: 'Flecha browser import helper', path: launcher, type: 'stdio', allowed_origins: ['chrome-extension://abdiaepfgbbhdjaecjipmaailijaaide/'] }, null, 2) + '\n');
if (windows) execFileSync('reg.exe', ['ADD', `HKCU\\Software\\Chromium\\NativeMessagingHosts\\${name}`, '/ve', '/t', 'REG_SZ', '/d', manifest, '/f'], { stdio: 'inherit' });
console.log(`Installed Flecha helper for ${profile}`);
