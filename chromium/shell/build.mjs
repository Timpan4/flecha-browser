// Builds the shell extension into chromium/shell/dist. `node build.mjs --watch` rebuilds on
// save; reload the extension (or the sidebar) to see the change. No Chromium build involved.
import { build, context } from 'esbuild';
import fs from 'node:fs';
import path from 'node:path';

const here = path.dirname(new URL(import.meta.url).pathname);
const root = path.resolve(here, '../..');
const out = path.join(here, 'dist');
const renderer = path.join(root, 'src/renderer');

function copyStatic() {
  fs.mkdirSync(path.join(out, 'fonts'), { recursive: true });
  fs.copyFileSync(path.join(here, 'manifest.json'), path.join(out, 'manifest.json'));
  for (const f of fs.readdirSync(renderer)) if (f.endsWith('.css')) fs.copyFileSync(path.join(renderer, f), path.join(out, f));
  fs.copyFileSync(
    path.join(root, 'node_modules/@fontsource-variable/jetbrains-mono/files/jetbrains-mono-latin-wght-normal.woff2'),
    path.join(out, 'fonts/jetbrains-mono.woff2'),
  );
  // The Electron sidebar page, pointed at the shell's bundle, once per surface.
  const html = fs
    .readFileSync(path.join(renderer, 'index.html'), 'utf8')
    .replace('src="app.js"', 'src="sidebar.js"')
    .replace('<link rel="stylesheet" href="onboarding.css" />', '$&\n    <link rel="stylesheet" href="shell.css" />');
  const pages = { window: 'window.html', panel: 'sidebar.html', newtab: 'newtab.html' };
  for (const [surface, page] of Object.entries(pages)) {
    fs.writeFileSync(path.join(out, page), html.replace('<html>', `<html data-surface="${surface}">`));
  }
  fs.copyFileSync(path.join(here, 'shell.css'), path.join(out, 'shell.css'));
  for (const f of ['settings.html', 'settings-shell.css']) fs.copyFileSync(path.join(here, f), path.join(out, f));
}

const options = {
  entryPoints: { background: path.join(here, 'background.ts'), sidebar: path.join(here, 'sidebar.ts'), settings: path.join(here, 'settings.ts') },
  bundle: true,
  format: 'esm',
  target: 'chrome140',
  outdir: out,
  logLevel: 'warning',
  plugins: [{ name: 'static', setup: (b) => b.onEnd(copyStatic) }],
};

if (process.argv.includes('--watch')) {
  const ctx = await context(options);
  await ctx.watch();
  console.log('shell: watching');
} else {
  await build(options);
}
