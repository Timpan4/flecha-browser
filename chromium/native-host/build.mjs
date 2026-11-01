// Bundles the native messaging host into one file, chromium/native-host/dist/flecha-host.cjs,
// which install-native-host.sh points Chromium at. Needs Node 22.5+ to run (node:sqlite).
import { build } from 'esbuild';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

await build({
  entryPoints: [path.join(here, 'host.ts')],
  outfile: path.join(here, 'dist/flecha-host.cjs'),
  bundle: true,
  platform: 'node',
  format: 'cjs',
  target: 'node22',
  logLevel: 'warning',
});
