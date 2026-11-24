#!/usr/bin/env node
// Renders the shipping shell's chrome (chromium/shell/ui) headless with fixture.js standing in
// for its hub, then pairs each render with the rewrite's `verify-slice.sh visual` capture.
//
//   node tools/visual-parity/shipping.mjs <out-dir> [<rewrite-captures>]
//
// Reads chromium/ only. The browser is Playwright's headless shell, started with an empty
// environment, a scratch HOME and no display. Needs `npm ci` at the repository root (esbuild).
import { execFileSync, spawnSync } from 'node:child_process';
import fs from 'node:fs';
import { createRequire } from 'node:module';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../../..');
const shell = path.join(root, 'chromium/shell');
const ui = path.join(shell, 'ui');
const [out, captures] = process.argv.slice(2);

if (!out) throw new Error('Usage: shipping.mjs <out-dir> [<rewrite-captures>]');

const browser = path.join(os.homedir(), '.cache/ms-playwright/chromium_headless_shell-1243/chrome-headless-shell-linux64/chrome-headless-shell');

if (!fs.existsSync(browser)) throw new Error(`The headless shell is missing: ${browser}`);

const site = path.join(out, 'site');

fs.rmSync(site, { recursive: true, force: true });

fs.mkdirSync(path.join(site, 'fonts'), { recursive: true });

const { build } = createRequire(path.join(root, 'package.json'))('esbuild');

// The bundles build.mjs makes, without the hub: fixture.js provides window.flecha and friends.
await build({
  entryPoints: { app: path.join(ui, 'app.ts'), settings: path.join(ui, 'settings.ts'), flechinha: path.join(ui, 'flechinha.ts'), historyPage: path.join(shell, 'historyPage.ts') },
  external: ['./tabsPane.js', './vsrPane.js'],
  bundle: true,
  format: 'esm',
  target: 'chrome140',
  outdir: site,
  logLevel: 'warning',
});

for (const file of fs.readdirSync(ui)) if (file.endsWith('.css')) fs.copyFileSync(path.join(ui, file), path.join(site, file));

for (const file of ['shell.css', 'settings-shell.css', 'history.css']) fs.copyFileSync(path.join(shell, file), path.join(site, file));

fs.copyFileSync(
  path.join(root, 'node_modules/@fontsource-variable/jetbrains-mono/files/jetbrains-mono-latin-wght-normal.woff2'),
  path.join(site, 'fonts/jetbrains-mono.woff2'),
);

fs.copyFileSync(path.join(here, 'fixture.js'), path.join(site, 'fixture.js'));

// A file:// page has no 'self' for the policy to allow, so the pages go without it.
const page = (html, bundle) =>
  html
    .replace(/<meta\s+http-equiv="Content-Security-Policy"[^>]*\/>/s, '')
    .replace(/<script type="module" src="[^"]+"><\/script>/, `<script src="fixture.js"></script>\n    <script type="module" src="${bundle}"></script>`);

fs.writeFileSync(
  path.join(site, 'window.html'),
  page(fs.readFileSync(path.join(ui, 'index.html'), 'utf8'), 'app.js')
    .replace('<html>', '<html data-surface="window">')
    .replace('<link rel="stylesheet" href="onboarding.css" />', '$&\n    <link rel="stylesheet" href="shell.css" />'),
);

fs.writeFileSync(path.join(site, 'settings.html'), page(fs.readFileSync(path.join(shell, 'settings.html'), 'utf8'), 'settings.js'));

fs.writeFileSync(path.join(site, 'flechinha.html'), page(fs.readFileSync(path.join(ui, 'flechinha.html'), 'utf8'), 'flechinha.js'));

fs.writeFileSync(path.join(site, 'history.html'), page(fs.readFileSync(path.join(shell, 'history.html'), 'utf8'), 'historyPage.js'));

// Each surface the rewrite captures, at the size of its capture.
const surfaces = [
  ['sidebar', 'window.html', [1278, 1024]],
  ['command-bar', 'window.html', [1278, 1024]],
  ['settings', 'settings.html', [1278, 1024]],
  ['history', 'history.html', [1278, 1024]],
  ['zoom', 'window.html', [1278, 1024]],
  ['context-menu', 'window.html', [1278, 1024]],
  ['downloads', 'window.html', [1278, 1024]],
  ['site-controls', 'window.html', [1278, 1024]],
  ['peek', 'window.html', [1278, 1024]],
  ['split', 'window.html', [1278, 1024]],
  ['flechinha', 'flechinha.html', [900, 700]],
];

const home = fs.mkdtempSync(path.join(out, 'home-'));
const shots = path.join(out, 'shipping');

fs.mkdirSync(shots, { recursive: true });

for (const [surface, file, [width, height]] of surfaces) {
  for (const theme of ['light', 'dark']) {
    const shot = path.join(shots, `shipping-${surface}-${theme}.png`);
    const url = `${pathToFileURL(path.join(site, file))}?surface=${surface}&theme=${theme}`;

    const run = spawnSync(
      browser,
      [
        '--no-first-run',
        '--disable-gpu',
        '--hide-scrollbars',
        '--allow-file-access-from-files',
        `--user-data-dir=${path.join(home, 'profile')}`,
        // Blink's PreferredColorScheme: 0 is dark, 1 is light.
        `--blink-settings=preferredColorScheme=${theme === 'dark' ? 0 : 1}`,
        '--virtual-time-budget=3000',
        `--window-size=${width},${height}`,
        `--screenshot=${shot}`,
        '--enable-logging=stderr',
        '--v=0',
        url,
      ],
      { env: { HOME: home, PATH: '/usr/bin:/bin', LANG: 'C.UTF-8' }, encoding: 'utf8', timeout: 60_000 },
    );

    const console = (run.stderr ?? '').split('\n').filter((line) => line.includes('CONSOLE') || line.includes('Uncaught'));

    if (run.status !== 0 || !fs.existsSync(shot)) throw new Error(`${surface} ${theme}: exit ${run.status}\n${run.stderr}`);
    console.forEach((line) => process.stderr.write(`${surface} ${theme}: ${line}\n`));
    process.stdout.write(`rendered ${shot}\n`);
  }
}

fs.rmSync(home, { recursive: true, force: true });

// Side by side: the shipping render left, the rewrite right; Light above Dark.
if (captures) {
  for (const [surface] of surfaces) {
    const pairs = ['light', 'dark'].flatMap((theme) => [
      ['-label', `Shipping (headless) ${theme}`, path.join(shots, `shipping-${surface}-${theme}.png`)],
      ['-label', `Rewrite ${theme}`, path.join(captures, `visual-${surface}-${theme}.png`)],
    ]);

    if (pairs.some(([, , file]) => !fs.existsSync(file))) continue;
    const target = path.join(out, `${surface}.png`);
    execFileSync('magick', ['montage', ...pairs.flat(), '-tile', '2x2', '-geometry', '+8+8', '-pointsize', '20', '-background', '#888888', target]);
    process.stdout.write(`paired ${target}\n`);
  }

  // Chromium draws its own find bar in the shipping browser, so there is no shell markup to
  // render: the rewrite's capture stands alone, checked against the token table instead.
  for (const surface of ['find']) {
    const shots = ['light', 'dark'].map((theme) => ['-label', `Rewrite ${theme} (shipping: Chromium-native)`, path.join(captures, `visual-${surface}-${theme}.png`)]);

    if (shots.some(([, , file]) => !fs.existsSync(file))) continue;
    const target = path.join(out, `${surface}.png`);
    execFileSync('magick', ['montage', ...shots.flat(), '-tile', '2x1', '-geometry', '+8+8', '-pointsize', '20', '-background', '#888888', target]);
    process.stdout.write(`labelled ${target}\n`);
  }
}
