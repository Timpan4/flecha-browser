import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { once, EventEmitter } from 'node:events';
import { copyFileSync, createWriteStream, existsSync, readdirSync, readFileSync, renameSync, utimesSync } from 'node:fs';
import { createServer as createHttpsServer } from 'node:https';
import { join, relative } from 'node:path';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';
import { crc32, deflateSync } from 'node:zlib';

import { blankRestartColor, downloadBody, fixture, openerColors } from './fixture.mjs';
import { checkProcessEnvironment } from './verify-process-env.mjs';

const out = join(process.env.HOME, '..');
const x11Driver = join(out, 'x11-input');
const mode = process.argv[2] ?? 'slice';

// Points in the main window with the visual mode's 260px sidebar and one favorite, measured from
// its captures. Xvfb has no compositor, so GPUI keeps the platform's title bar and draws no
// window controls: the menu button sits right of the sidebar toggle (ui/styles.css:32, 55, 66).
// The window menu drops 4px below it (settingsUi.ts:202), a 5px-padded list of 25.5px rows with
// 11px separators (shell.css:78, 88, 132-133); the Space menu opens at the pointer, and its
// Separate Cookies and Logins row follows Edit Space, New Folder, Open in New Window and a separator.
const menuButton = [10 + 30 + 2 + 15, 10 + 14];
const newWindowRow = [82, 61];
const zoomRow = [82, 287];
const spaceHeader = [60, 157];
const spaceMenuItem = [80, Math.round(5 + 3 * 25.5 + 11 + 25.5 / 2)];

// The per-step allowance from chromium/tests/native-workflows.mjs.
const stepTimeoutMs = 10_000;

const shippingTrace = readFileSync(new URL('./slice-ts-replay.jsonl', import.meta.url), 'utf8')
  .trim().split('\n').map(line => JSON.parse(line));

assert(['slice', 'open', 'focus', 'popups', 'adblock', 'incognito', 'fullscreen', 'break-restore', 'clipboard', 'visual', 'tab-status'].includes(mode), 'unknown slice check');

const easylist = '[Adblock Plus 2.0]\n! Title: EasyList\n/fixture-ad$xmlhttprequest\n';
const easyprivacy = '[Adblock Plus 2.0]\n! Title: EasyPrivacy\n/fixture-tracker$xmlhttprequest\n';
let listResponse = 'offline';
const listRequests = new Set();

// The tab-status check's colours: the icon /icon declares, and `--blue`, which draws the
// loading spinner (chromium/shell/ui/styles.css:702-709, ui/tokens.css:36).
const faviconColor = '#D6336C';
const spinnerColor = '#3AA6D9';
// Responses to /slow, held open so the page stays loading until the check releases them.

const slowResponses = [];

/** A `size` square PNG in one colour. */
function solidPng(size, color) {
  const chunk = (type, data) => {
    const body = Buffer.concat([Buffer.from(type, 'latin1'), data]);
    const length = Buffer.alloc(4);
    const crc = Buffer.alloc(4);
    length.writeUInt32BE(data.length);
    crc.writeUInt32BE(crc32(body));

    return Buffer.concat([length, body, crc]);
  };

  const header = Buffer.alloc(13);
  header.writeUInt32BE(size, 0);
  header.writeUInt32BE(size, 4);
  header[8] = 8; // bits per channel
  header[9] = 2; // RGB
  const rgb = [1, 3, 5].map(at => parseInt(color.slice(at, at + 2), 16));
  const row = Buffer.from([0, ...Array.from({ length: size }, () => rgb).flat()]);
  const pixels = Buffer.concat(Array.from({ length: size }, () => row));

  return Buffer.concat([
    Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
    chunk('IHDR', header), chunk('IDAT', deflateSync(pixels)), chunk('IEND', Buffer.alloc(0)),
  ]);
}

// Each page's head, then its body. Blink reads icon links from the head only.
const statusPages = {
  '/icon': ['<link rel="icon" href="/icon.png">', ''],
  // The click is the user gesture an AudioContext needs to start.
  '/audio': ['', `<button id="play">Play</button><script>
    document.querySelector('#play').addEventListener('click', () => {
      const context = new AudioContext();
      const tone = context.createOscillator();
      tone.connect(context.destination);
      tone.start();
    });
  </script>`],
};

// The partition check's icons: /partition-icon.png answers in the colour of the partition whose
// cookie the request carries, one colour for each icon URL.
const partitionColors = { r1: '#7B2FBE', i1: '#E8871E', i2: '#C21F8A', r2: '#2E6FD8', s1: '#1E9E5A' };

const site = await fixture((path, response, request) => {
  if (path === '/icon.png') {
    response.writeHead(200, { 'Content-Type': 'image/png', 'Cache-Control': 'no-store' }).end(solidPng(32, faviconColor));

    return true;
  }

  const query = new URL(request.url, 'http://localhost').searchParams;

  // Names the page's partition in a cookie, then declares that site's icon.
  if (path === '/partition-icon') {
    response.writeHead(200, {
      'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store',
      'Set-Cookie': `flecha_icon=${query.get('mark')}; Path=/`,
    }).end(`<!doctype html><title>Flecha fixture /partition-icon</title>
      <link rel="icon" href="/partition-icon.png?n=${query.get('n')}">`);

    return true;
  }

  if (path === '/partition-icon.png') {
    const mark = /(?:^|; )flecha_icon=([a-z])(?:;|$)/.exec(request.headers.cookie ?? '')?.[1];
    const color = partitionColors[`${mark}${query.get('n')}`];

    if (color) response.writeHead(200, { 'Content-Type': 'image/png', 'Cache-Control': 'no-store' }).end(solidPng(32, color));
    else response.writeHead(404).end();

    return true;
  }

  if (path === '/slow') {
    slowResponses.push(response);

    return true;
  }

  if (statusPages[path]) {
    response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8', 'Cache-Control': 'no-store' }).end(`<!doctype html>
      <title>Flecha fixture ${path}</title>
      ${statusPages[path][0]}
      <style>
        html { background: #147a70; color: white; font: 24px system-ui; }
        body { margin: 32px; }
        #marker { width: 116px; height: 56px; background: #ffcc49; }
        #play { display: block; width: 320px; height: 64px; margin: 8px 0; font: inherit; }
      </style>
      <div id="marker"></div>
      ${statusPages[path][1]}`);

    return true;
  }

  if (path !== '/easylist.txt' && path !== '/easyprivacy.txt') return false;

  listRequests.add(path);

  if (listResponse === 'offline') {
    response.destroy();

    return true;
  }

  const body = path === '/easyprivacy.txt' ? easyprivacy : listResponse === 'corrupt'
    ? easylist.replace('! Title:', '! Checksum: bad\n! Title:').replace('fixture-ad', 'replacement')
    : easylist;

  response.writeHead(200, { 'Content-Length': Buffer.byteLength(body) }).end(body);

  return true;
});

const { base } = site;
const pass = message => console.log(`PASS ${message}`);
let waitingFor = 'start Flecha';

function browser(name, fixturePort) {
  const env = { ...process.env };

  if (fixturePort !== undefined) env.FLECHA_VERIFY_ADBLOCK_PORT = String(fixturePort);

  const child = spawn('./target/debug/flecha', ['--verify'], { env });
  const log = createWriteStream(join(out, `${name}.log`));
  const emitter = new EventEmitter();
  const events = [];
  let seq = 0;
  let exit;
  const closed = once(child, 'close');
  child.stderr.pipe(log, { end: false });
  createInterface({ input: child.stdout }).on('line', line => {
    log.write(line + '\n');

    if (line.startsWith('phase1 ')) {
      events.push(JSON.parse(line.slice('phase1 '.length)));
      emitter.emit('message');
    }
  });
  child.once('close', (code, signal) => {
    exit = { code, signal };
    log.end();
    emitter.emit('message');
  });
  child.stdin.on('error', error => {
    if (error.code !== 'EPIPE') throw error;
  });

  async function wait(description, match, since = 0, timeoutMs) {
    waitingFor = description;
    const signal = timeoutMs === undefined ? undefined : AbortSignal.timeout(timeoutMs);

    for (;;) {
      const event = events.slice(since).find(match);

      if (event) return event;
      assert(!exit, `Flecha exited before ${description}: ${JSON.stringify(exit)}`);
      await once(emitter, 'message', { signal });
    }
  }

  /** The reply to one request, which may carry an error. */
  async function attempt(op, detail = {}) {
    const request = ++seq;
    const sent = events.length;
    child.stdin.write(JSON.stringify({ seq: request, op, ...detail }) + '\n');
    // Commands run one at a time, so a rejection after this write is this request's.

    const reply = await wait(`reply to ${op}`, event =>
      (event.type === 'reply' && event.seq === request) || event.type === 'invalid_request', sent);

    assert.notEqual(reply.type, 'invalid_request', `Flecha does not understand the ${op} request`);

    return reply;
  }

  async function command(op, detail = {}) {
    const reply = await attempt(op, detail);
    assert.equal(reply.error, undefined, reply.error);

    return reply.result;
  }

  return {
    events, attempt, command, wait, pid: child.pid,
    async exited() {
      child.stdin.end();
      assert.deepEqual(await closed, [0, null]);
    },
    loaded: (tab, path, since = 0) => wait(
      `HTTP load of ${path}`,
      event => event.type === 'loaded' && event.tab === tab && event.url === base + path,
      since,
    ).then(load => assert.equal(load.status, 200, path)),
    async quit() {
      await command('quit');
      child.stdin.end();
      assert.deepEqual(await closed, [0, null]);
    },
    async present(window) {
      const after = events.length;
      await command('capture', window ? { window } : {});
      await wait('GPUI presented the fixture frame', event => event.type === 'presented', after);
    },
    release() {
      child.stdin.destroy();
      child.stdout.destroy();
      child.stderr.destroy();
      child.unref();
    },
  };
}

async function splitCapture(app, name, snapshot) {
  await app.present();
  await capture(name, ['#147A70', '#FFCC49', '#703496', '#F386B9']);

  // Use solid fixture frames for a complete pixel mask. Keep the original
  // capture for viewing the labelled pages.
  const { split, splitRatio = 0.5 } = active(snapshot);
  const colors = ['#01B7DD', '#F93520'];

  for (const [index, tab] of split.entries()) {
    const label = `${name}-solid-${index}`;
    const settled = site.probe(label);
    const after = app.events.length;
    await app.command('script', {
      tab,
      source: `
        document.documentElement.style.backgroundColor = '${colors[index]}';
        document.body.style.visibility = 'hidden';
        requestAnimationFrame(() => requestAnimationFrame(() => fetch('/probe/${label}?value=ready')));
      `,
    });
    assert.equal(await settled, 'ready');
    await app.wait('CEF paint of solid fixture', event => event.type === 'paint' && event.tab === tab, after);
  }

  await app.present();

  const geometry = execFileSync(x11Driver, ['geometry', 'Flecha'], { encoding: 'utf8' })
    .trim().split(' ').map(Number);

  assert(geometry.length === 4 && geometry.every(Number.isInteger) && geometry[2] > 0 && geometry[3] > 0,
    'invalid X11 window geometry');

  await captureUntil(`${name}-geometry`, 'root', path => paneMismatch(path, geometry, splitRatio, colors));

  for (const tab of split) {
    await app.command('script', {
      tab,
      source: "document.documentElement.style.removeProperty('background-color'); document.body.style.removeProperty('visibility');",
    });
  }
}

function paneMismatch(path, geometry, ratio, colors) {
  const dimensions = execFileSync('magick', ['identify', '-format', '%w %h', path], { encoding: 'utf8' });
  const [width, height] = dimensions.trim().split(' ').map(Number);
  const bytes = width * height * 3; // ImageMagick's 8-bit RGB stream.
  const pixels = execFileSync('magick', [path, '-alpha', 'off', '-depth', '8', 'rgb:-'], { maxBuffer: bytes });
  assert.equal(pixels.length, bytes, 'split screenshot RGB size');

  const pixel = (x, y) => pixels.readUIntBE((y * width + x) * 3, 3);
  const [left, right] = colors.map(color => Number.parseInt(color.slice(1), 16));
  // The default Space's tint in Dark and Light (flecha-ui/src/theme.rs). The default
  // System theme follows the desktop's appearance, which GPUI reads as Light on
  // this session's private bus, where no desktop portal answers.
  const sidebars = [0x2f3746, 0xcdd9f7];
  // The page area sits kFlechaGap in from the window's top, right and bottom and flush with
  // the docked sidebar (patch 0010); split panes keep kFlechaSplitGap between them and share
  // the rest by the ratio (patch 0019).
  const pageGap = 8;
  const splitGap = 8;
  // Each card's page shadow (patch 0041: 4px down, 14px blur) shades its neighbour's edge.
  // GPUI's blur fades out by about twice its radius, so pane colours are exact only past that.
  const shadow = 4 + 2 * 14;
  const [windowX, windowY, windowWidth, windowHeight] = geometry;
  const middle = windowY + Math.floor(windowHeight / 2);

  if (!sidebars.includes(pixel(windowX + 2, middle))) return 'could not locate the sidebar';

  let pageLeft = windowX;

  while (pageLeft < windowX + windowWidth && pixel(pageLeft, middle) !== left) pageLeft++;

  if (pageLeft >= windowX + windowWidth) return 'could not locate the left pane';

  const pageTop = windowY + pageGap;
  const pageBottom = windowY + windowHeight - pageGap;
  const pageRight = windowX + windowWidth - pageGap;
  // Fractional edges rasterize either way in the one column or row they cross.
  const leftEnd = pageLeft + (pageRight - pageLeft - splitGap) * ratio;
  const rightStart = leftEnd + splitGap;
  const inside = (x, y, x0, x1) => x >= x0 - 1 && x < x1 + 1 && y >= pageTop - 1 && y < pageBottom + 1;
  const solid = (x, y, x0, x1) => x >= x0 + shadow && x < x1 - shadow && y >= pageTop + shadow && y < pageBottom - shadow;

  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const actual = pixel(x, y);
      const where = `in ${width}×${height}, panes ${pageLeft}–${leftEnd.toFixed(1)} and ${rightStart.toFixed(1)}–${pageRight}, rows ${pageTop}–${pageBottom}`;

      if (actual === left && !inside(x, y, pageLeft, leftEnd)) return `left pane colour outside its pane at (${x}, ${y}) ${where}`;

      if (actual === right && !inside(x, y, rightStart, pageRight)) return `right pane colour outside its pane at (${x}, ${y}) ${where}`;

      if (solid(x, y, pageLeft, leftEnd) && actual !== left) return `unexpected #${actual.toString(16)} at (${x}, ${y}) in the left pane ${where}`;

      if (solid(x, y, rightStart, pageRight) && actual !== right) return `unexpected #${actual.toString(16)} at (${x}, ${y}) in the right pane ${where}`;
    }
  }
}

async function capture(name, colors, window = 'root') {
  await captureUntil(name, window, path => {
    const histogram = execFileSync('magick', [path, '-format', '%c', 'histogram:info:'], { encoding: 'utf8' });
    const missing = colors.filter(color => !histogram.includes(color));

    if (!missing.length) return;

    if (name === 'restart-split' && histogram.includes(blankRestartColor)) {
      return 'blank restart control rendered its replacement frame';
    }

    return `missing fixture colors ${missing.join(', ')}`;
  });
}

async function captureUntil(name, window, mismatch, signal) {
  waitingFor = `${name} capture`;
  const path = join(out, `${name}.png`);
  let previous;

  // X can still expose the previous frame after GPUI acknowledges presentation.
  // The launcher watchdog bounds both the capture attempts and their subprocesses.
  for (;;) {
    signal?.throwIfAborted();
    execFileSync('magick', ['import', '-window', window, path]);
    const reason = mismatch(path);

    if (!reason) {
      if (previous) console.log(`READY ${name} capture`);

      return;
    }

    if (reason !== previous) {
      console.error(`RETRY ${name}: ${reason}`);
      previous = reason;
    }

    await delay(100, undefined, { signal }); // Matches Spike 1's capture readiness polling.
  }
}

function win(snapshot) {
  return snapshot.core.windows.find(window => window.id === snapshot.window);
}

function active(snapshot) {
  const window = win(snapshot);

  return window.spaces[window.activeSpaceId];
}

function compareShipping(step, snapshot, names) {
  const space = active(snapshot);
  const name = id => names.get(id);
  const actual = { step, today: space.today.map(name), active: name(space.activeTabId), split: space.split?.map(name) ?? null };
  assert.deepEqual(actual, shippingTrace.find(entry => entry.step === step), `${step} state differs from the shipping TS shell`);
}

function engine(snapshot, tab, kind, window) {
  const page = snapshot.browsers.find(page => page.tab === tab);
  assert(page?.browser > 0, `expected a live CEF browser: ${tab}`);
  assert.deepEqual(page.owner, { kind, window });

  return page.browser;
}

async function document(app, tab, label, prepare = '') {
  const reply = site.probe(label);
  await app.command('script', {
    tab,
    source: `${prepare}
      fetch('/probe/${label}?value=' + encodeURIComponent(JSON.stringify({
        token: window.documentToken,
        draft: document.querySelector('input').value,
      })), { cache: 'no-store' });`,
  });
  waitingFor = `fixture DOM probe ${label}`;

  return JSON.parse(await reply);
}

async function nativeInput(app, tab) {
  execFileSync('cc', [
    '-std=c17', '-Wall', '-Wextra', '-Werror', 'spikes/gpui-cef-compositor/x11-input.c',
    '-DFLECHA_VERIFY_CURSOR', '-lX11', '-lXtst', '-lXfixes', '-o', x11Driver,
  ]);
  const center = site.probe('input-center');
  const typed = site.probe('native-key');

  const marker = execFileSync('magick', [
    join(out, 'open.png'), '-transparent', '#FFCC49', '-alpha', 'extract', '-negate',
    '-trim', '-format', '%X %Y', 'info:',
  ], { encoding: 'utf8' }).trim().split(' ').map(Number);

  assert.equal(marker.length, 2);
  assert(marker.every(Number.isFinite));
  await app.command('script', {
    tab,
    source: `
      const field = document.querySelector('input');
      const { x, y, width, height } = field.getBoundingClientRect();
      const marker = document.querySelector('#marker').getBoundingClientRect();
      field.addEventListener('input', () => fetch('/probe/native-key?value=' + encodeURIComponent(field.value)), { once: true });
      fetch('/probe/input-center?value=' + encodeURIComponent(JSON.stringify([x + width / 2 - marker.x, y + height / 2 - marker.y])));
    `,
  });
  waitingFor = 'fixture input geometry';
  const [x, y] = JSON.parse(await center);
  execFileSync(x11Driver, ['click', Math.round(marker[0] + x), Math.round(marker[1] + y)]);
  execFileSync(x11Driver, ['key', 'x']);
  waitingFor = 'X11 key delivery to the CEF page';
  assert.equal(await typed, 'x');
  pass('an X11 click and key passed through GPUI into the CEF input field');
}

async function nativeContextMenu(app) {
  const [x, y] = execFileSync('magick', [
    join(out, 'open.png'), '-transparent', '#FFCC49', '-alpha', 'extract', '-negate',
    '-trim', '-format', '%X %Y', 'info:',
  ], { encoding: 'utf8' }).trim().split(' ').map(Number);

  assert(Number.isFinite(x) && Number.isFinite(y), 'missing page marker');

  // The menu opens at the click, over the page's yellow marker. A point inside it, past its 8px
  // corner radius and 5px padding, shows `--surface` (ui/tokens.css:51, 107) while it is open:
  // Theme::System follows Xvfb's Light appearance, Dark is accepted too.
  const probe = [x + 20, y + 12];
  const pixel = path => execFileSync('magick', [path, '-format', `%[hex:p{${probe[0]},${probe[1]}}]`, 'info:'], { encoding: 'utf8' }).trim().slice(0, 6).toUpperCase();
  const surfaces = ['FFFFFF', '161E2B'];

  assert.equal(pixel(join(out, 'open.png')), 'FFCC49', 'the menu probe is not on the page marker');
  execFileSync(x11Driver, ['right-click', x + 1, y + 1]);
  await app.present();
  await captureUntil('page-context-menu', 'root', path =>
    surfaces.includes(pixel(path)) ? undefined : `the menu surface is missing (${pixel(path)})`);
  execFileSync(x11Driver, ['key', 'Escape']);
  await app.present();
  await captureUntil('page-context-menu-dismissed', 'root', path =>
    pixel(path) === 'FFCC49' ? undefined : `the page marker is still covered (${pixel(path)})`);
  pass('right-click opened the GPUI page menu and Escape dismissed it');
}

/**
 * The tab row's right-click menu, its middle-click close, and the command bar's Tab-for-actions,
 * on the one Today tab `left` the slice starts with. Each check duplicates `left` and closes the
 * copy again, so the slice goes on from the same Today list.
 */
async function tabRowInput(app, left) {
  // `left`'s row, the only one in Today: the sidebarFocus geometry's top row (y 235).
  const row = [95, 235];
  const today = async () => active(await app.command('snapshot')).today;

  const duplicated = async label => {
    const [copy, ...rest] = await poll(label, async () => {
      const tabs = await today();

      return tabs.length === 2 ? tabs : undefined;
    });

    const state = await app.command('snapshot');

    assert.deepEqual(rest, [left]);
    assert.equal(state.core.tabs[copy].url, state.core.tabs[left].url);
    assert.equal(active(state).activeTabId, copy);

    return copy;
  };

  const closeTop = async label => {
    // GPUI hit-tests the last frame it drew, which must show the copy in the top row.
    await app.present();
    execFileSync(x11Driver, ['middle-click', ...row]);
    await poll(label, async () => {
      const tabs = await today();

      return tabs.length === 1 && tabs[0] === left;
    });
  };

  execFileSync(x11Driver, ['right-click', ...row]);
  await poll('the tab menu to open', async () => {
    const { error } = await app.attempt('menu_row', { label: 'Duplicate' });

    // Until the tab menu opens, the request falls through to the page menu.
    if (error === 'no page menu is open') return false;
    assert.equal(error, undefined);

    return true;
  });
  await duplicated('the tab menu to duplicate the tab');
  pass('right-clicking a tab row opened its menu, and Duplicate opened a copy above it');
  await closeTop('middle-click to close the copy');
  pass('middle-clicking a tab row closed that tab');

  // An empty bar's Tab lists the actions; one letter then matches Duplicate Tab first, which
  // among pages it would not, so Enter runs it only in the actions mode.
  execFileSync(x11Driver, ['chord', 'Control_L', 't']);
  execFileSync(x11Driver, ['key', 'Tab']);
  execFileSync(x11Driver, ['key', 'd']);
  execFileSync(x11Driver, ['key', 'Return']);
  await duplicated('the Duplicate Tab action from the actions mode');
  await closeTop('middle-click to close the action copy');
  pass('Tab in an empty command bar searched the actions, and Enter ran Duplicate Tab');
}

/** Ctrl+Alt+1–9 (Ctrl+1–9 on macOS, `fixed_shortcuts`) switch straight to the numbered Space. */
async function spaceNumberKeys(app, [first, second]) {
  execFileSync(x11Driver, ['focus', 'Flecha']);
  // The footer's New Space left its editor open, and an open editor keeps its keys.
  execFileSync(x11Driver, ['key', 'Escape']);

  for (const [key, space] of [['2', second], ['1', first]]) {
    execFileSync(x11Driver, ['chord', 'Control_L', 'Alt_L', key]);
    await poll(`Ctrl+Alt+${key} to switch Spaces`, async () =>
      win(await app.command('snapshot')).activeSpaceId === space);
  }

  pass('Ctrl+Alt+2 and Ctrl+Alt+1 switched to the second Space and back');
}

async function browserDialogs(app) {
  const tab = await openTab(app, '/unload');
  execFileSync(x11Driver, ['focus', 'Flecha']);
  const key = name => execFileSync(x11Driver, ['key', name]);
  const close = () => execFileSync(x11Driver, ['chord', 'Control_L', 'w']);

  const dialog = (tab, after) => app.wait('CEF browser dialog', event =>
    event.type === 'dialog_changed' && event.tab === tab, after, stepTimeoutMs);

  const scriptDialog = async (label, expression, keys, expected) => {
    const reply = site.probe(label, stepTimeoutMs);
    reply.catch(() => {});
    const after = app.events.length;
    await app.command('script', {
      tab,
      source: `fetch('/probe/${label}?value=' + encodeURIComponent(String(${expression})))`,
    });
    await dialog(tab, after);
    await app.present();
    keys.forEach(key);
    assert.equal(await reply, expected);
  };

  await scriptDialog('js-alert', "alert('Flecha alert')", ['Return'], 'undefined');
  await scriptDialog('js-confirm', "confirm('Flecha confirm')", ['Escape'], 'false');
  await scriptDialog('js-prompt-default', "prompt('Flecha prompt', 'default')", ['Return'], 'default');
  await scriptDialog('js-prompt-edit', "prompt('Flecha prompt', 'default')", ['z', 'Return'], 'z');
  pass('CEF alert, confirm and prompt waited for GPUI and received the chosen result');

  let after = app.events.length;
  const uploadCancelled = site.probe('upload-cancelled', stepTimeoutMs);
  uploadCancelled.catch(() => {});
  await app.command('script', {
    tab,
    source: `{
      const draft = document.querySelector('input');
      const upload = document.createElement('input');
      upload.type = 'file';
      document.body.append(upload);
      upload.addEventListener('cancel', () => fetch('/probe/upload-cancelled?value=cancelled'), { once: true });
      draft.addEventListener('click', () => upload.showPicker(), { once: true });
    }`,
  });
  await clickElement(app, tab, 'input[aria-label=Draft]', ['#147A70', '#FFCC49'], 'dialog-upload-gesture');
  await dialog(tab, after);
  await app.present();
  key('Escape');
  assert.equal(await uploadCancelled, 'cancelled');
  pass('an upload reached the GPUI picker and cancellation returned to the web page');

  await clickElement(app, tab, 'input:not([type=file])', ['#147A70', '#FFCC49'], 'dialog-unload-gesture');
  after = app.events.length;
  close();
  await dialog(tab, after);
  await app.present();
  close();
  await app.present();
  assert((await app.command('snapshot')).core.tabs[tab], 'A second close bypassed beforeunload');
  key('Escape');
  await app.present();
  assert((await app.command('snapshot')).core.tabs[tab], 'Choosing Stay should keep the beforeunload tab open');
  after = app.events.length;
  close();
  await dialog(tab, after);
  await app.present();
  key('Return');
  await app.wait('beforeunload tab closure', event =>
    event.type === 'closed_by_page' && event.tab === tab, after, stepTimeoutMs);
  pass('beforeunload Stay kept the tab and Leave closed it');

  after = app.events.length;
  const auth = active(await app.command('open', { url: base + '/basic-auth' })).activeTabId;
  await dialog(auth, after);
  await app.present();
  ['u', 'Tab', 'p', 'Return'].forEach(key);
  await app.loaded(auth, '/basic-auth', after);
  pass('an HTTP Basic challenge received the username and masked password entered in GPUI');

  after = app.events.length;
  await app.command('script', { tab: auth, source: "confirm('Close me')" });
  await dialog(auth, after);
  await app.present();
  close();
  await app.wait('tab closure with a pending dialog', event =>
    event.type === 'closed' && event.tab === auth, after, stepTimeoutMs);
  pass('closing a tab dismissed its pending modal and closed the CEF browser');
}

async function rejectInvalidCertificate(app) {
  const key = join(out, 'test-tls.key');
  const cert = join(out, 'test-tls.pem');

  execFileSync('openssl', [
    'req', '-x509', '-newkey', 'rsa', '-nodes', '-config', '/dev/null', '-subj', '/CN=127.0.0.1',
    '-addext', 'subjectAltName=IP:127.0.0.1', '-keyout', key, '-out', cert,
  ], { stdio: 'ignore' });
  let requested = false;

  const server = createHttpsServer({ key: readFileSync(key), cert: readFileSync(cert) }, (_, response) => {
    requested = true;
    response.end('untrusted fixture');
  });

  server.listen(0, '127.0.0.1');
  await once(server, 'listening');

  try {
    const url = `https://127.0.0.1:${server.address().port}/untrusted`;
    const after = app.events.length;
    const tab = active(await app.command('open', { url })).activeTabId;

    await app.wait('CEF certificate error', event =>
      event.type === 'security_changed' && event.tab === tab, after, stepTimeoutMs);
    await app.wait('the untrusted load to fail', event =>
      event.type === 'load_failed' && event.tab === tab && event.url === url, after, stepTimeoutMs);
    assert.equal(requested, false, 'the request reached the untrusted HTTPS server');
    pass('CEF reported the untrusted certificate and cancelled the HTTPS request');
  } finally {
    server.close();
  }
}

async function sidebarResizeAndReveal(app, tab) {
  const width = (await app.command('snapshot')).core.sidebarWidth;

  const [x, y, , height] = execFileSync(x11Driver, ['geometry', 'Flecha'], { encoding: 'utf8' })
    .trim().split(' ').map(Number);

  const middle = y + Math.floor(height / 2);
  const move = clientX => execFileSync(x11Driver, ['move', x + clientX, middle]);

  const sidebar = path => {
    const pixel = execFileSync('magick', [path, '-format', `%[hex:p{${x + 20},${middle}}]`, 'info:'], { encoding: 'utf8' });

    return ['CDD9F7', '2F3746'].includes(pixel.trim().slice(0, 6).toUpperCase());
  };

  const pageWidth = async label => {
    const reply = site.probe(label, stepTimeoutMs);
    reply.catch(() => {});
    await app.command('script', { tab, source: `fetch('/probe/${label}?value=' + innerWidth)` });

    return Number(await reply);
  };

  const originalPage = await pageWidth('before-sidebar-drag');
  execFileSync(x11Driver, ['press', x + width - 2, middle]);

  try {
    move(width + 38);
    await app.present();
    assert.equal((await app.command('snapshot')).core.sidebarWidth, width, 'sidebar width saved before release');
    assert.equal(await pageWidth('during-sidebar-drag'), originalPage - 40, 'the page did not resize while dragging');
  } finally {
    execFileSync(x11Driver, ['release']);
  }

  await poll('the sidebar width to save', async () => (await app.command('snapshot')).core.sidebarWidth === width + 40);
  const saved = JSON.parse(JSON.parse(readFileSync(join(process.env.FLECHA_PROFILE, 'session', 'state.json'), 'utf8')));
  assert.equal(saved.sidebarWidth, width + 40);
  pass('drag resized the CEF viewport live and persisted the new width on mouse up');

  execFileSync(x11Driver, ['chord', 'Control_L', 's']);
  await app.present();
  await captureUntil('sidebar-collapsed', 'root', path => sidebar(path) ? 'sidebar stayed docked' : undefined);
  const unoccluded = await pageWidth('collapsed-sidebar-width');
  move(2);
  await app.present();
  await captureUntil('sidebar-hover-reveal', 'root', path => sidebar(path) ? undefined : 'floating sidebar missing');
  assert.equal(await pageWidth('floating-sidebar-width'), unoccluded, 'the floating sidebar resized the page');
  move(width + 80);
  await captureUntil('sidebar-hover-leave', 'root', path => sidebar(path) ? 'floating sidebar stayed open' : undefined);
  execFileSync(x11Driver, ['chord', 'Control_L', 's']);
  pass('the collapsed sidebar revealed over the page at the left edge and hid when the pointer left');
}

async function quickWins(app) {
  await app.command('new_space');

  const tab = await openTab(app, '/open');

  const [x, y, , height] = execFileSync(x11Driver, ['geometry', 'Flecha'], { encoding: 'utf8' })
    .trim().split(' ').map(Number);

  // Default sidebar width and page top inset, also used by the visual parity check.
  const pageX = x + 260;
  const pageY = y + 8;
  const input = (operation, px, py) => execFileSync(x11Driver, [operation, Math.round(px), Math.round(py)]);
  const cursor = () => execFileSync(x11Driver, ['cursor'], { encoding: 'utf8' }).trim().split(' ').map(Number);

  const captureWhile = (name, check) => poll(name, () => {
    const path = join(out, `${name}.png`);

    execFileSync('magick', ['import', '-window', 'root', path]);

    return check(path) && path;
  });

  const pixels = (path, crop, format, output = 'info:') => execFileSync('magick', [
    path, '-crop', crop, '+repage', '-format', format, output,
  ], { encoding: 'utf8' }).trim().toUpperCase();

  const boxes = site.probe('quick-wins-boxes', stepTimeoutMs);
  await app.command('script', { tab, source: `
    const link = document.querySelector('a');
    const marker = document.querySelector('#marker');
    link.href = '/split';
    link.removeAttribute('target');
    marker.style.cursor = 'none';
    const center = element => {
      const { x, y, width, height } = element.getBoundingClientRect();
      return [x + width / 2, y + height / 2];
    };
    const centers = [link, document.querySelector('input'), marker].map(center);
    fetch('/probe/quick-wins-boxes?value=' + encodeURIComponent(JSON.stringify(centers)));
  ` });
  const [link, field, hidden] = JSON.parse(await boxes).map(([left, top]) => [pageX + left, pageY + top]);
  const blank = [pageX + 400, pageY + 300];
  input('move', ...blank);
  await app.present();

  const [arrow] = cursor();

  input('move', ...link);

  const [hand] = await poll('X11 link cursor', () => {
    const result = cursor();

    return result[1] && result[0] !== arrow && result;
  });

  const statusPixel = `1x1+${pageX + 4}+${y + height - 8 - 12}`;

  const linkShot = await captureWhile('quick-wins-link-status', path =>
    ['FFFFFF', '161E2B'].includes(pixels(path, statusPixel, '%[hex:p{0,0}]').slice(0, 6)));

  input('move', ...field);
  await poll('X11 text cursor', () => {
    const [serial, visible] = cursor();

    return visible && serial !== arrow && serial !== hand;
  });
  await captureWhile('quick-wins-no-link-status', path =>
    pixels(path, statusPixel, '%[hex:p{0,0}]').startsWith('147A70'));
  input('move', ...hidden);
  await poll('X11 CSS cursor none', () => cursor()[1] === 0);
  input('move', ...link);
  await poll('X11 cursor returns to link hand', () => cursor()[0] === hand);
  pass('X11 changed cursor over links and text, hid it over cursor:none, and drew and cleared the link status pill');

  const dragReady = site.probe('quick-wins-drag-ready', stepTimeoutMs);
  const dragLeave = site.probe('quick-wins-drag-leave', stepTimeoutMs);
  await app.command('script', { tab, source: `
    document.querySelector('input').value = 'Drag to select this text';
    document.documentElement.addEventListener('mouseleave', event => {
      fetch('/probe/quick-wins-drag-leave?value=' + event.buttons);
    }, { once: true });
    fetch('/probe/quick-wins-drag-ready?value=ready');
  ` });
  await dragReady;
  input('press', ...field);

  try {
    input('move', field[0] - 8, field[1]);
    input('move', pageX - 8, field[1]);
    assert.equal(await dragLeave, '1', 'CEF received the page exit without the held left button');
  } finally {
    execFileSync(x11Driver, ['release']);
  }

  pass('X11 drag selection kept the left button pressed on exit into the sidebar');

  for (const [operation, position, path] of [['click', link, '/split'], ['back', blank, '/open'], ['forward', blank, '/split']]) {
    const after = app.events.length;

    input(operation, ...position);
    await app.wait(`${operation} loaded ${path}`, event =>
      event.type === 'loaded' && event.tab === tab && event.url === base + path, after, stepTimeoutMs);
  }

  pass('X11 side buttons navigated back and forward in the active tab');

  // The address bar's two 26px actions sit against the 4px inner pad and 6px sidebar pad.
  const copy = [x + 260 - 6 - 4 - 26 - 13, y + 10 + 28 + 8 + 18];
  input('move', ...copy);
  await app.present();
  const addressCrop = `186x20+${x + 28}+${copy[1] - 10}`;
  const address = path => pixels(path, addressCrop, '%#');
  const before = address(await captureWhile('quick-wins-address', () => true));
  input('click', ...copy);
  const flashShot = await captureWhile('quick-wins-copy-flash', path => address(path) !== before);
  assert.equal((await app.command('clipboard')).text, base + '/split');
  await captureWhile('quick-wins-copy-restored', path => address(path) === before);
  pass('Copy link wrote the active URL to GPUI clipboard and the confirmation cleared');

  const title = 'MMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMM';
  const after = app.events.length;
  await app.command('script', { tab, source: `document.title = '${title}'` });
  await app.wait('long sidebar title', event => event.type === 'title' && event.tab === tab && event.title === title, after);
  const rowY = y + 235; // The first Today row in the one-Space layout used by sidebarFocus.
  // This strip is the gap between the shortened title and the menu button on hover.
  const titleCrop = `7x18+${x + 186}+${rowY - 9}`;
  const hasTitleInk = path => Number(pixels(path, titleCrop, '%k')) > 1;
  input('move', ...blank);
  const resting = await captureWhile('quick-wins-row-resting', hasTitleInk);
  input('move', x + 80, rowY);
  const hovered = await captureWhile('quick-wins-row-hovered', path => !hasTitleInk(path));
  pass('the resting tab title occupied the space taken by controls on hover');

  input('click', x + 234, y + 173);
  await poll('Today Clear to archive the tab', async () => {
    const snapshot = await app.command('snapshot');

    return !active(snapshot).today.length && snapshot.core.archive.some(entry => entry.url === base + '/split');
  });
  await app.wait('CEF closure after clearing Today', event => event.type === 'closed' && event.tab === tab);
  pass('Today Clear archived the tab and closed its CEF browser');

  if (existsSync('/mnt/visual')) {
    for (const path of [linkShot, flashShot, resting, hovered]) copyFileSync(path, join('/mnt/visual', relative(out, path)));
  }
}

async function keyDelivery(app, tabs, key, step, expected) {
  const received = site.probe(`${step}-key`);

  for (const [name, tab] of Object.entries(tabs)) {
    const ready = site.probe(`${step}-${name}-ready`);

    await app.command('script', {
      tab,
      source: `
        window.probeKeys = [];
        addEventListener('keydown', event => {
          if (event.key !== '${key}') return;
          window.probeKeys.push(event.key);
          fetch('/probe/${step}-key?value=${name}');
        });
        fetch('/probe/${step}-${name}-ready?value=ready');
      `,
    });
    assert.equal(await ready, 'ready');
  }

  waitingFor = `X11 key after ${step}`;
  execFileSync(x11Driver, ['key', key]);
  const actual = await received;

  assert.equal(actual, expected, `${step} sent ${key} to pane ${actual}, expected ${expected}`);

  for (const [name, tab] of Object.entries(tabs)) {
    const report = site.probe(`${step}-${name}-report`);

    await app.command('script', {
      tab,
      source: `fetch('/probe/${step}-${name}-report?value=' + JSON.stringify(window.probeKeys));`,
    });
    assert.deepEqual(JSON.parse(await report), name === expected ? [key] : [], `keys received by pane ${name}`);
  }

  pass(`${step} sent ${key} only to pane ${expected}`);
}

async function waitForSelection(app, tab, description) {
  for (;;) {
    const state = await app.command('snapshot');

    if (active(state).activeTabId === tab) return state;
    waitingFor = description;
    await delay(100); // Matches the capture poll.
  }
}

async function sidebarFocus(app, left) {
  const opened = await app.command('open', { url: base + '/split' });
  const right = active(opened).activeTabId;

  await app.loaded(right, '/split');
  await app.command('select', { tab: left });
  await app.present();
  await capture('focus-left', ['#147A70', '#FFCC49']);

  // Xvfb fixture geometry with one Space, measured from the focus-left capture: the page, then
  // pane B's row (centred at y 235, ending at x 256) and its Split button, which sits left of
  // the 22px menu and close buttons with the row's 10px gaps and 6px right padding
  // (ui/styles.css:582, 586, 631).
  execFileSync(x11Driver, ['click', 340, 110]);
  execFileSync(x11Driver, ['move', 169, 235]);
  await app.present();
  execFileSync(x11Driver, ['click', 169, 235]);
  const split = await waitForSelection(app, right, 'sidebar Split selected pane B');

  assert.deepEqual(active(split).split, [left, right]);
  await keyDelivery(app, { A: left, B: right }, 'y', 'sidebar split', 'B');

  // The split pair replaces both rows at the first row's position, with pane A on the left.
  execFileSync(x11Driver, ['click', 50, 235]);
  await waitForSelection(app, left, 'sidebar selected pane A');
  await keyDelivery(app, { A: left, B: right }, 'z', 'sidebar select', 'A');

  const ready = site.probe('command-keys-ready');
  await app.command('script', { tab: left, source: `
    window.commandKeys = [];
    addEventListener('keydown', event => {
      if (['ArrowDown', 'Escape'].includes(event.key)) window.commandKeys.push(event.key);
    });
    fetch('/probe/command-keys-ready?value=ready');
  ` });
  assert.equal(await ready, 'ready');
  execFileSync(x11Driver, ['chord', 'Control_L', 'l']);
  execFileSync(x11Driver, ['key', 'Down']);
  execFileSync(x11Driver, ['key', 'Escape']);
  await keyDelivery(app, { A: left, B: right }, 'u', 'command bar dismissed', 'A');
  const commandKeys = site.probe('command-keys');
  await app.command('script', { tab: left, source: `fetch('/probe/command-keys?value=' + JSON.stringify(window.commandKeys));` });
  assert.deepEqual(JSON.parse(await commandKeys), [], 'command bar keys reached pane A');
  await managerFocus(app, left, right);
}

async function managerFocus(app, left, right) {
  const ready = site.probe('manager-keys-ready');
  await app.command('script', { tab: left, source: `
    window.managerKeys = [];
    addEventListener('keydown', event => window.managerKeys.push(event.key));
    fetch('/probe/manager-keys-ready?value=ready');
  ` });
  assert.equal(await ready, 'ready');
  execFileSync(x11Driver, ['chord', 'Control_L', 'comma']);
  await app.present();

  const [x, y] = execFileSync(x11Driver, ['geometry', 'Flecha'], { encoding: 'utf8' })
    .trim().split(' ').map(Number);

  // Tabs & Spaces, the second item in the Settings nav column beside the sidebar: General's
  // highlight spans y 62-91 in the visual capture, and items sit 2px apart (ui/settings.css:56).
  execFileSync(x11Driver, ['click', x + 360, y + 108]);
  // The selected Settings tab's accent (theme.rs accent_strong), Dark or Light: Theme::System
  // follows Xvfb's Light appearance.
  await captureUntil('tabs-spaces-manager', 'root', path => {
    const histogram = execFileSync('magick', [path, '-format', '%c', 'histogram:info:'], { encoding: 'utf8' });

    return ['#73B4E4', '#1F6FB5'].some(color => histogram.includes(color)) ? undefined : 'the Tabs & Spaces tab is not selected';
  });

  execFileSync(x11Driver, ['key', 'q']);
  execFileSync(x11Driver, ['chord', 'Control_L', 'a']);

  for (const key of 'split') execFileSync(x11Driver, ['key', key]);
  execFileSync(x11Driver, ['key', 'Down']);
  execFileSync(x11Driver, ['key', 'Delete']);

  for (;;) {
    const snapshot = await app.command('snapshot');

    if (!snapshot.core.tabs[right]) {
      assert(snapshot.core.tabs[left], 'the search excluded the other split tab');
      assert(snapshot.core.archive.some(tab => tab.url === base + '/split'), 'the matching tab reached the Archive');
      break;
    }

    waitingFor = 'archive the tab matching the manager search';
    await delay(100);
  }

  execFileSync(x11Driver, ['key', 'Escape']);
  const keys = site.probe('manager-keys');
  await app.command('script', { tab: left, source: `fetch('/probe/manager-keys?value=' + JSON.stringify(window.managerKeys));` });
  assert.deepEqual(JSON.parse(await keys), [], 'manager search and list keys reached CEF');
  await keyDelivery(app, { A: left }, 'g', 'Tabs & Spaces dismissed', 'A');
}

// Restore Spaces From Before Import through the command bar of the regular window, whose active
// tab `left` shows `path`.
async function restoreSpaces(app, left, path = '/open') {
  const session = join(process.env.FLECHA_PROFILE, 'session');
  copyFileSync(join(session, 'state.json'), join(session, 'before-import.json'));
  const before = await app.command('snapshot');
  const changed = await app.command('new_space');
  assert.equal(changed.core.spaces.length, before.core.spaces.length + 1);
  execFileSync(x11Driver, ['focus', 'Flecha']);
  execFileSync(x11Driver, ['chord', 'Control_L', 'l']);

  for (const key of 'restore') execFileSync(x11Driver, ['key', key]);
  execFileSync(x11Driver, ['key', 'Return']);
  await app.present();
  execFileSync('magick', ['import', '-window', 'root', join(out, 'restore-spaces-prompt.png')]);

  const [x, y] = execFileSync(x11Driver, ['geometry', 'Flecha'], { encoding: 'utf8' })
    .trim().split(' ').map(Number);

  // The Restore button on the Import pane's message card, beside Cancel, measured from the
  // restore-spaces-prompt capture: the pane starts right of the sidebar and the Settings nav.
  const after = app.events.length;
  execFileSync(x11Driver, ['click', x + 629, y + 143]);
  await app.loaded(left, path, after);
  const restored = await app.command('snapshot');
  assert.deepEqual(restored.core.spaces, before.core.spaces);
  assert.equal(active(restored).activeTabId, left);
  execFileSync(x11Driver, ['key', 'Escape']);
  pass('the command bar confirmed restore, removed the newly added Space, and reopened the backup tab');
}

async function cookie(app, expected, op = 'open') {
  const after = app.events.length;
  const snapshot = await app.command(op, { url: base + '/cookie/check' });
  const tab = op === 'peek' ? snapshot.peeks[0].tab : active(snapshot).activeTabId;
  await app.loaded(tab, '/cookie/check', after);

  const result = await app.wait('cookie fixture response', event =>
    event.type === 'title' && event.tab === tab && ['Cookie present', 'Cookie absent', 'Shared cookie'].includes(event.title), after);

  assert.equal(result.title, expected, 'cookie request used the wrong Space context');

  return tab;
}

async function isolateWithTransients(app, space) {
  const isIsolated = snapshot => snapshot.core.spaces.find(entry => entry.id === space).isolated;
  const clickedPeek = await cookie(app, 'Shared cookie', 'peek');
  await app.present();
  await capture('peek-backdrop', ['#147A70', '#FFCC49']);

  // The footer's New Space button: inside the sidebar's 6px right and 8px bottom padding, half
  // the 28px icon button from each (ui/styles.css:32, 136), measured from the visual captures.
  const [x, y, , height] = execFileSync(x11Driver, ['geometry', 'Flecha'], { encoding: 'utf8' })
    .trim().split(' ').map(Number);

  const newSpace = [x + 260 - 6 - 14, y + height - 8 - 14];
  const spaces = snapshot => snapshot.core.spaces.length;
  const spacesBefore = spaces(await app.command('snapshot'));
  execFileSync(x11Driver, ['click', ...newSpace]);

  for (;;) {
    const state = await app.command('snapshot');
    assert.equal(spaces(state), spacesBefore, 'Peek backdrop passed the click through to New Space');
    assert.equal(isIsolated(state), false);

    if (!state.peeks.length) break;
    await delay(100); // Matches the capture poll.
  }

  await app.wait('CEF closure after clicking the Peek backdrop', event =>
    event.type === 'closed' && event.tab === clickedPeek);
  // Without Peek the same click adds a Space, so the backdrop, not a miss, held it back. GPUI
  // hit-tests the last frame it drew, so that frame must be the one without the backdrop.
  await app.present();
  execFileSync(x11Driver, ['click', ...newSpace]);
  await poll('New Space from the footer button', async () =>
    spaces(await app.command('snapshot')) === spacesBefore + 1);
  await app.command('switch', { space });
  pass('clicking New Space through the Peek backdrop dismissed Peek and left the Spaces alone');

  await cookie(app, 'Shared cookie');
  const [{ window: mini, tab: detached }] = (await app.command('detach')).flechinhas;
  const peek = await cookie(app, 'Shared cookie', 'peek');
  const before = await app.command('snapshot');
  engine(before, detached, 'flechinha', mini);
  engine(before, peek, 'peek', before.window);
  assert(before.nativeWindows.includes(mini), 'GPUI opened Flechinha before switching its Space partition');

  // Dispatch isolation directly so the modal cannot hide a stale request context.
  const after = await app.command('isolate');
  assert(isIsolated(after));
  assert.deepEqual(after.peeks, [], 'partition change left a Peek on the previous cookie jar');
  assert.deepEqual(after.flechinhas, [], 'partition change left a Flechinha on the previous cookie jar');

  for (const tab of [detached, peek]) {
    assert(!after.browsers.some(browser => browser.tab === tab), 'transient browser survived its partition change');
    await app.wait('CEF closed a transient from the previous partition', event =>
      event.type === 'closed' && event.tab === tab);
  }

  assert(!(await app.command('snapshot')).nativeWindows.includes(mini), 'GPUI closed Flechinha after its partition changed');
  pass('changing a Space partition closed both its Peek and Flechinha CEF browsers');

  return after;
}

// Clicks the element's centre with X11, which gives the page a user gesture.
async function clickElement(app, tab, selector, colors, label, window) {
  await app.present(window);
  await capture(label, colors);

  const marker = execFileSync('magick', [
    join(out, `${label}.png`), '-transparent', colors[1], '-alpha', 'extract', '-negate',
    '-trim', '-format', '%X %Y', 'info:',
  ], { encoding: 'utf8' }).trim().split(' ').map(Number);

  assert(marker.length === 2 && marker.every(Number.isFinite), `no marker for ${label}`);
  const offset = site.probe(`${label}-offset`);
  await app.command('script', {
    tab,
    source: `{
      const marker = document.querySelector('#marker').getBoundingClientRect();
      const target = document.querySelector('${selector}').getBoundingClientRect();
      fetch('/probe/${label}-offset?value=' + JSON.stringify([
        target.x + target.width / 2 - marker.x, target.y + target.height / 2 - marker.y,
      ]));
    }`,
  });
  waitingFor = `geometry of ${selector}`;
  const [x, y] = JSON.parse(await offset);
  execFileSync(x11Driver, ['click', Math.round(marker[0] + x), Math.round(marker[1] + y)]);
}

async function openTab(app, path) {
  const after = app.events.length;
  const tab = active(await app.command('open', { url: base + path })).activeTabId;
  await app.loaded(tab, path.split('?')[0], after);

  return tab;
}

async function tabDrags(app) {
  const space = win(await app.command('new_space')).activeSpaceId;
  const pinned = await openTab(app, '/opener');
  await app.command('pin', { tab: pinned });
  const favorite = await openTab(app, '/closed');
  await app.command('favorite', { tab: favorite });
  await app.command('new_folder', { name: 'Drag fixtures' });
  const a = await openTab(app, '/open');
  const b = await openTab(app, '/split');
  const c = await openTab(app, '/peek');
  await app.present();
  execFileSync(x11Driver, ['focus', 'Flecha']);

  const [x, y, width, height] = execFileSync(x11Driver, ['geometry', 'Flecha'], { encoding: 'utf8' })
    .trim().split(' ').map(Number);

  const pointer = (op, [localX, localY]) => execFileSync(x11Driver, [op, x + localX, y + localY]);

  // Sidebar dimensions from tokens.rs at the default sidebar width. The drag adds the 4px
  // Pinned end target. Each visible row, including a split pair, takes 32px plus its margins.
  const pinnedHeader = 10 + 28 + 8 + 36 + 8 + 44 + 8 + 26 + 8;
  const firstPinned = pinnedHeader + 22 + 1;
  const rowHeight = 32 + 2;

  const pinTop = index => firstPinned + index * rowHeight;

  const todayTop = (index, pinnedRows, dragging = false) =>
    pinTop(pinnedRows) + (dragging ? 4 : 0) + 8 + 30 + 34 + index * rowHeight;

  const today = (index, pinnedRows = 2) => [60, todayTop(index, pinnedRows) + 16];
  const beforeToday = index => [60, todayTop(index, 2, true) + 2];
  const folder = index => [60, pinTop(index) + 16];
  const favoriteTile = (index = 0) => [36 + 62 * index, 112];
  const pane = fraction => [260 + Math.round((width - 260) * fraction), Math.floor(height / 2)];
  const snapshot = () => app.command('snapshot');
  const pins = state => state.core.spaces.find(candidate => candidate.id === space).pinnedFlat;
  const pinOrder = state => pins(state).filter(node => node.depth === 0).map(node => node.tabId ?? node.id);
  const children = state => pins(state).filter(node => node.depth === 1).map(node => node.tabId);

  const folderId = pins(await snapshot()).find(node => node.name === 'Drag fixtures').id;

  const drag = async (label, from, to, preview) => {
    waitingFor = label;
    pointer('press', from);

    try {
      pointer('move', [from[0] + 8, from[1]]);
      await app.present();
      pointer('move', to);
      await app.present();

      if (preview) await preview();
    } finally {
      execFileSync(x11Driver, ['release']);
    }

    await app.present();
    waitingFor = label;

    return snapshot();
  };

  const retain = name => {
    if (existsSync('/mnt/visual')) copyFileSync(join(out, name + '.png'), join('/mnt/visual', name + '.png'));
  };

  const cue = (name, rgb, crop) => async () => {
    await captureUntil(name, 'root', path => {
      const pixels = execFileSync('magick', [path, '-crop', crop, '+repage', '-depth', '8', 'rgb:-']);

      // Xvfb's composited color differed from the opaque blue token by up to three channel values.
      for (let index = 0; index < pixels.length; index += 3) {
        if (rgb.every((channel, offset) => Math.abs(channel - pixels[index + offset]) <= 3)) return;
      }

      return 'the highlighted border did not appear in ' + crop;
    }, AbortSignal.timeout(stepTimeoutMs));
    retain(name);
  };

  pointer('click', today(1));
  await app.present();
  const clicked = active(await snapshot());
  assert.equal(clicked.activeTabId, b);
  assert.deepEqual(clicked.today, [c, b, a]);
  assert.equal(clicked.split, undefined);

  const insertionCrop = '64x6+' + (x + 185) + '+' + (y + todayTop(0, 2, true) - 1);
  let state = await drag('reorder Today', today(2), beforeToday(0), cue('drag-insertion', [58, 166, 217], insertionCrop));
  assert.deepEqual(active(state).today, [a, c, b]);
  assert.equal(active(state).activeTabId, b);
  pass('a click selected its tab and a drag reordered Today with a visible insertion line');

  const viewportCrop = '64x6+' + (x + 300) + '+' + (y + 8);
  state = await drag('drop on the left of the viewport', today(0), pane(0.25), cue('drag-split-preview', [31, 111, 181], viewportCrop));
  assert.deepEqual(active(state).split, [a, b]);
  assert.equal(active(state).activeTabId, a);
  await capture('drag-split-pair', ['#147A70', '#703496']);
  retain('drag-split-pair');
  state = await drag('drop on the right of the existing split', today(1), pane(0.75));
  assert.deepEqual(active(state).split, [a, c]);
  assert.equal(active(state).activeTabId, c);
  state = await drag('split onto a pinned row', today(1), [125, pinTop(0) + 16]);
  assert.deepEqual(active(state).split, [pinned, b]);
  pass('viewport drops showed a highlight, created and replaced a split, and a sidebar row drop split with that tab');

  const beforeCancel = active(state);
  state = await drag('cancel over Favorites', today(0), favoriteTile(), () => {
    execFileSync(x11Driver, ['key', 'Escape']);
  });
  assert.deepEqual(active(state), beforeCancel);
  assert.deepEqual(state.core.favorites, [favorite]);
  state = await drag('drop outside the tab targets', today(0), [60, height - 100]);
  assert.deepEqual(active(state), beforeCancel);
  pass('Escape and an outside drop cancelled the drag without changing tabs');

  state = await drag('Today to Pinned', today(0), [60, pinTop(0) + 2]);
  assert.deepEqual(pinOrder(state), [a, pinned, folderId]);
  state = await drag('reorder Pinned', folder(1), [60, pinTop(0) + 2]);
  assert.deepEqual(pinOrder(state), [pinned, a, folderId]);
  state = await drag('Pinned into a folder', folder(1), folder(2));
  assert.deepEqual(children(state), [a]);
  state = await drag('Today into a folder', today(0, 3), folder(1));
  assert.deepEqual(children(state), [a, c]);
  state = await drag('reorder folder children', folder(3), [65, pinTop(2) + 2]);
  assert.deepEqual(children(state), [c, a]);
  state = await drag('folder to Favorites', folder(2), favoriteTile());
  assert.deepEqual(state.core.favorites, [c, favorite]);
  state = await drag('reorder Favorites', favoriteTile(1), favoriteTile());
  assert.deepEqual(state.core.favorites, [favorite, c]);
  state = await drag('Favorites to Pinned', favoriteTile(1), [60, pinnedHeader + 11]);
  assert.deepEqual(pinOrder(state), [pinned, folderId, c]);
  state = await drag('folder to Today', folder(2), [60, todayTop(0, 4, true) - 17]);
  assert.deepEqual(active(state).today, [b, a]);
  state = await drag('Pinned to Today', folder(2), [60, todayTop(0, 3, true) - 17]);
  assert.deepEqual(active(state).today, [b, a, c]);
  assert.deepEqual(children(state), []);
  pass('reordered Pinned, a folder and Favorites, and moved tabs between all sidebar sections');

  return state.core;
}

// HTML fullscreen: the page alone in its window without the sidebar, which comes back when Esc
// or a tab switch ends it. Xvfb runs no window manager, so the native fullscreen request has
// no effect here and the window keeps its size.
async function pageFullscreen(app, main, other) {
  const sidebars = ['#2F3746', '#CDD9F7'];
  const page = ['#147A70', '#FFCC49'];
  const tab = await openTab(app, '/fullscreen');
  const histogram = path => execFileSync('magick', [path, '-format', '%c', 'histogram:info:'], { encoding: 'utf8' });
  const sidebarShown = path => sidebars.some(color => histogram(path).includes(color));

  const enter = async label => {
    const after = app.events.length;

    await clickElement(app, tab, '#full', page, label, main);
    await app.wait(`${label}: the page to enter fullscreen`, event =>
      event.type === 'fullscreen' && event.tab === tab && event.fullscreen, after);
    await app.present(main);
    await captureUntil(`${label}-shown`, 'root', path => {
      if (sidebarShown(path)) return 'the sidebar stayed beside the fullscreen page';

      return page.every(color => histogram(path).includes(color)) ? undefined : 'the fullscreen page is not on screen';
    });
  };

  const ended = async (step, after) => {
    await app.wait(`${step} to end fullscreen`, event =>
      event.type === 'fullscreen' && event.tab === tab && !event.fullscreen, after);
    await app.present(main);
    await captureUntil(`fullscreen-${step.replaceAll(/\W/g, '-')}`, 'root', path =>
      sidebarShown(path) ? undefined : 'the sidebar did not come back');
  };

  await enter('fullscreen');
  pass('a page entered HTML fullscreen through CEF and the window showed it without the sidebar');

  let after = app.events.length;

  execFileSync(x11Driver, ['key', 'Escape']);
  await ended('Esc', after);
  pass('Esc asked the page to leave fullscreen and the sidebar came back');

  await enter('fullscreen-again');
  after = app.events.length;
  await app.command('select', { tab: other });
  await ended('a tab switch', after);
  pass('switching tabs ended the page\'s fullscreen and the sidebar came back');
  await app.quit();
}

// The tab row's status marks as ui/app.ts:543-590 draws them: the icon the page declares once it
// downloads, a spinner in its place while the page loads, and on a page that plays sound a
// speaker that mutes the page when clicked.
async function tabStatus(app, main, left) {
  const [windowX, windowY, , windowHeight] = execFileSync(x11Driver, ['geometry', 'Flecha'], { encoding: 'utf8' })
    .trim().split(' ').map(Number);

  const sidebarWidth = (await app.command('snapshot')).core.sidebarWidth;
  const region = `${sidebarWidth}x${windowHeight}+${windowX}+${windowY}`;

  const crop = (name, path) => {
    const cropped = join(out, `${name}-sidebar.png`);
    execFileSync('magick', [path, '-crop', region, '+repage', cropped]);

    // isolated-run.sh binds FLECHA_VISUAL_OUT here; the scratch directory goes when the run passes.
    if (existsSync('/mnt/visual')) copyFileSync(cropped, join('/mnt/visual', `${name}.png`));

    return cropped;
  };

  const histogram = path => execFileSync('magick', [path, '-format', '%c', 'histogram:info:'], { encoding: 'utf8' });

  const sidebar = async (name, mismatch) => {
    await app.present(main);
    await captureUntil(name, 'root', path => mismatch(histogram(crop(name, path))));
  };

  const tab = async id => (await app.command('snapshot')).core.tabs[id];
  const browser = async id => (await app.command('snapshot')).browsers.find(page => page.tab === id);

  let after = app.events.length;
  const iconTab = await openTab(app, '/icon');
  // Blink reports the default /favicon.ico until the parser reaches the page's own icon.
  await app.wait('the page to declare its icon', event => event.type === 'favicon_urls' && event.tab === iconTab &&
    event.urls.length === 1 && event.urls[0] === base + '/icon.png', after);
  await poll('the tab to keep its downloaded icon', async () => (await tab(iconTab)).favicon === base + '/icon.png');
  await sidebar('tab-favicon', colors => colors.includes(faviconColor) ? undefined : 'the row shows no site icon');
  pass('the tab kept the icon its page declared, and its row drew the downloaded image');

  await app.command('select', { tab: left });
  await sidebar('tab-idle', colors => colors.includes(spinnerColor) ? 'blue in the sidebar with no page loading' : undefined);
  after = app.events.length;
  const slowTab = active(await app.command('open', { url: base + '/slow' })).activeTabId;
  await app.wait('the slow page to start loading', event =>
    event.type === 'loading' && event.tab === slowTab && event.loading, after);
  // Away from the slow tab, so the address bar's loading line is not on screen.
  await app.command('select', { tab: left });
  await sidebar('tab-loading', colors => colors.includes(spinnerColor) ? undefined : 'no spinner in the loading row');
  after = app.events.length;

  for (const response of slowResponses.splice(0)) {
    response.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' }).end('<!doctype html><title>Flecha fixture /slow</title>');
  }

  await app.loaded(slowTab, '/slow', after);
  await sidebar('tab-loaded', colors => colors.includes(spinnerColor) ? 'the spinner stayed after the load' : undefined);
  pass('a loading tab showed a spinner in its row that went away once the page loaded');

  const audioTab = await openTab(app, '/audio');
  await sidebar('tab-silent', colors => colors.includes(spinnerColor) ? 'the page is still loading' : undefined);
  const silent = join(out, 'tab-silent-sidebar.png');
  after = app.events.length;
  await clickElement(app, audioTab, '#play', ['#147A70', '#FFCC49'], 'audio-play', main);
  await app.wait('the page to become audible', event =>
    event.type === 'audible' && event.tab === audioTab && event.audible, after, stepTimeoutMs);
  assert.equal((await browser(audioTab)).audible, true);

  // The speaker is the right end of what the sidebar gained since the page was silent: the title
  // shortens to make room for it, so the change starts inside the title. The click goes half the
  // mark's 14px icon (ui/styles.css:2190) in from that end.
  let mark;
  await sidebar('tab-audible', () => {
    const changed = execFileSync('magick', [
      silent, join(out, 'tab-audible-sidebar.png'), '-compose', 'difference', '-composite',
      '-separate', '-evaluate-sequence', 'max', '-threshold', '0', '-format', '%@', 'info:',
    ], { encoding: 'utf8' }).trim();

    const [, width, height, x, y] = /^(\d+)x(\d+)\+(\d+)\+(\d+)$/.exec(changed)?.map(Number) ?? [];

    if (!width || !height) return 'no audio mark in the sidebar';
    mark = [windowX + x + width - 14 / 2, windowY + y + height / 2];
  });
  pass(`the audible page's row showed an audio mark (${mark.map(Math.round).join(', ')})`);

  // The split mark (ui/app.ts:574-576) sits where the close button shows on hover, and hovering
  // the row hides it (ui/styles.css:660-683). A split pair drawn side by side leaves it out
  // (ui/styles.css:1955-1958), so the audio tab is pinned first and its split partner stays in
  // Today. The pinned row is drawn alone and in a split, each time with the pointer on the page
  // beside the sidebar and then on the row's title. A row is 32px tall (ui/styles.css:6). The
  // split mark moves the speaker toward the title, so the title's own end is measured: it is the
  // last pixel before the 20px audio mark (ui/styles.css:2174) that differs from the row's fill,
  // within the line the 14px speaker icon (ui/styles.css:2190) spans, clear of the row's edges.
  const rowHeight = 32;
  const audibleCentre = Math.round(mark[1] - windowY);
  const beforeSpeaker = join(out, 'tab-audible-title.png');

  execFileSync('magick', [
    join(out, 'tab-audible-sidebar.png'),
    '-crop', `${Math.round(mark[0] - windowX - 20 / 2)}x14+0+${audibleCentre - 14 / 2}`,
    '+repage', beforeSpeaker,
  ]);

  const rowFill = execFileSync('magick', [beforeSpeaker, '-format', '%[pixel:p{w-1,h/2}]', 'info:'], { encoding: 'utf8' }).trim();

  const titleEnd = execFileSync('magick', [
    beforeSpeaker, '(', '+clone', '-fill', rowFill, '-colorize', '100', ')',
    '-compose', 'difference', '-composite',
    '-separate', '-evaluate-sequence', 'max', '-threshold', '0', '-format', '%@', 'info:',
  ], { encoding: 'utf8' }).trim();

  const [, endWidth, , endX] = /^(\d+)x(\d+)\+(\d+)\+(\d+)$/.exec(titleEnd)?.map(Number) ?? [];

  assert(endWidth, `no title before the audio mark: ${titleEnd}`);

  const titleWidth = endX + endWidth;
  const click = ([x, y]) => execFileSync(x11Driver, ['click', Math.round(x), Math.round(y)]);
  const awayPoint = [windowX + sidebarWidth + 40, windowY + windowHeight - 40];
  const titlePoint = centre => [windowX + titleWidth / 2, windowY + centre];

  const rowBand = (name, path, centre) => {
    const band = join(out, `${name}-row.png`);
    execFileSync('magick', [path, '-crop', `${sidebarWidth}x${rowHeight}+0+${centre - rowHeight / 2}`, '+repage', band]);

    return band;
  };

  // `magick compare` exits 0 for similar images, 1 for different ones and 2 on an error, and
  // writes its metric to stderr. Anything else fails the check rather than reading as a match.
  const compare = (...args) => {
    const run = spawnSync('magick', ['compare', ...args, 'null:'], { encoding: 'utf8' });

    assert(!run.error && (run.status === 0 || run.status === 1), `magick compare failed: ${run.error ?? run.stderr}`);

    const metric = Number(run.stderr.split(' ')[0]);

    assert(Number.isFinite(metric), `magick compare gave no metric: ${run.stderr}`);

    return { metric, output: run.stderr };
  };

  const differing = (a, b) => compare('-metric', 'AE', a, b).metric;

  const shot = async (name, check) => {
    await app.present(main);
    await captureUntil(name, 'root', path => check(crop(name, path)));
  };

  // Where `part` best matches in `path`, and whether it matches there exactly.
  const search = (path, metric, part) => {
    const { metric: different, output } = compare('-metric', metric, '-subimage-search', path, part);
    const [, x, y] = /@ (\d+),(\d+)/.exec(output) ?? [];

    assert(x !== undefined, `magick compare gave no match location: ${output}`);

    return { at: [Number(x), Number(y)], exact: different === 0 };
  };

  // A split pair drawn side by side keeps the speaker in the row's narrower half, and it still
  // mutes the page. The speaker is found by its 14px icon (ui/styles.css:2190).
  const speaker = join(out, 'split-speaker.png');

  execFileSync('magick', [
    join(out, 'tab-audible-sidebar.png'), '-crop', `14x14+${Math.round(mark[0] - windowX - 14 / 2)}+${audibleCentre - 14 / 2}`,
    '+repage', speaker,
  ]);

  // The row draws its actions after the speaker only while the pointer is on it, and an X11
  // click presses before GPUI draws that hover, so a click aimed at the resting speaker can land
  // on the close button. The pointer first rests on the title, and the clicks go where the
  // speaker is drawn then. A half of a split pair moves the icon by a fraction of a pixel, so the
  // match is the closest one, and the clicks show whether it was the speaker.
  const findSpeaker = path => search(path, 'RMSE', speaker)?.at;
  const restingX = Math.round(mark[0] - windowX - 14 / 2);
  let hoveredSpeaker;

  click(titlePoint(audibleCentre));
  await shot('tab-audible-hover', path => {
    const moved = findSpeaker(path);

    if (!moved || moved[0] >= restingX) return 'the hovered row made no room for its actions';
    hoveredSpeaker = [windowX + moved[0] + 14 / 2, windowY + moved[1] + 14 / 2];
  });
  click(hoveredSpeaker);
  await poll('the mark to mute the page', async () =>
    (await tab(audioTab)).muted === true && (await browser(audioTab)).muted);
  click(hoveredSpeaker);
  await poll('the mark to unmute the page', async () =>
    (await tab(audioTab)).muted === undefined && !(await browser(audioTab)).muted);
  pass('clicking the audio mark muted the page in CEF and the saved tab, and clicking again unmuted it');

  const tabsBefore = Object.keys((await app.command('snapshot')).core.tabs);

  await app.command('split', { url: base + '/split' });
  const partner = Object.keys((await app.command('snapshot')).core.tabs).find(id => !tabsBefore.includes(id));

  await app.command('select', { tab: audioTab });
  click(awayPoint);

  let resting;
  let pairSpeaker;

  await shot('split-pair', path => {
    resting = findSpeaker(path);

    return resting ? undefined : 'no match for the speaker';
  });

  // Until the pointer is on the row, the pair's actions float invisibly over its end, speaker
  // included. An X11 click presses before GPUI draws the hover, so the pointer first rests on
  // the title, and the speaker moves aside to make room for the actions.
  click([windowX + resting[0] / 2, windowY + resting[1] + 14 / 2]);
  await shot('split-pair-hover', path => {
    const moved = findSpeaker(path);

    if (!moved || moved[0] >= resting[0]) return 'the paired row made no room for its actions';
    pairSpeaker = [windowX + moved[0] + 14 / 2, windowY + moved[1] + 14 / 2];
  });
  click(pairSpeaker);
  await poll('the paired row\'s speaker to mute the page', async () => (await tab(audioTab)).muted === true);
  click(pairSpeaker);
  await poll('the paired row\'s speaker to unmute the page', async () => (await tab(audioTab)).muted === undefined);
  pass('a split pair drawn side by side kept the speaker, and it muted and unmuted the page');

  await app.command('select', { tab: partner });
  await app.command('close');
  await app.command('select', { tab: audioTab });

  // Pinning and the split's new tab move the row, so it is found again by its icon and title.
  const titleOnly = join(out, 'split-title.png');

  execFileSync('magick', [
    join(out, 'tab-audible-sidebar.png'), '-crop', `${titleWidth}x${rowHeight}+0+${audibleCentre - rowHeight / 2}`,
    '+repage', titleOnly,
  ]);

  const findRow = path => {
    const found = search(path, 'AE', titleOnly);

    return found?.exact ? found.at[1] + rowHeight / 2 : undefined;
  };

  await app.command('pin', { tab: audioTab });
  click(awayPoint);
  let aloneCentre;
  let alone;
  let aloneHovered;

  await shot('split-alone', path => {
    aloneCentre = findRow(path);

    if (aloneCentre === undefined) return 'the pinned audio row is not in the sidebar';
    alone = rowBand('split-alone', path, aloneCentre);
  });
  click(titlePoint(aloneCentre));
  await shot('split-alone-hover', path => {
    aloneHovered = rowBand('split-alone-hover', path, aloneCentre);

    return differing(alone, aloneHovered) ? undefined : 'the row shows no actions on hover';
  });

  // The close button is the right end of what hovering the row shows.
  const actions = execFileSync('magick', [
    alone, aloneHovered, '-compose', 'difference', '-composite',
    '-separate', '-evaluate-sequence', 'max', '-threshold', '0', '-format', '%@', 'info:',
  ], { encoding: 'utf8' }).trim();

  const [, actionsWidth, , actionsX] = /^(\d+)x(\d+)\+(\d+)\+(\d+)$/.exec(actions).map(Number);
  // Half the close button's 13px icon (ui/styles.css:648) in from that end.
  const closeX = windowX + actionsX + actionsWidth - 13 / 2;

  await app.command('split', { url: base + '/split' });
  await app.command('select', { tab: audioTab });
  click(awayPoint);
  let splitCentre;
  let split;

  await shot('split-joined', path => {
    splitCentre = findRow(path);

    if (splitCentre === undefined) return 'the audio row is not in the sidebar';
    split = rowBand('split-joined', path, splitCentre);

    if (!differing(split, aloneHovered)) return 'the row still shows its hover actions';

    return differing(split, alone) ? undefined : 'the split row shows no split mark';
  });
  click(titlePoint(splitCentre));
  await shot('split-joined-hover', path => differing(rowBand('split-joined-hover', path, splitCentre), aloneHovered)
    ? 'hovering the split row did not swap its split mark for the row actions'
    : undefined);
  pass('a tab in a split showed the split mark, and hovering its row swapped the mark for the row actions');

  // Closing a pinned tab closes its page, whatever happens to the pin.
  click([closeX, windowY + splitCentre]);
  await poll('the close button under the split mark to close the page', async () => !(await browser(audioTab)));
  pass('the close button under the split mark closed the page');
}


// Each partition fetches its own copy of a site's icon, since the response can depend on that
// partition's cookies: an icon one partition downloaded never shows in another. Each page names
// its partition in a cookie, and the icon answers in that partition's colour.
async function partitionIcons(app, main) {
  const histogram = path => execFileSync('magick', [path, '-format', '%c', 'histogram:info:'], { encoding: 'utf8' });

  const shows = (name, title, window, expected, foreign) => poll(`the ${title} window to draw ${name}`, async () => {
    execFileSync(x11Driver, ['focus', title]);
    await app.present(window);

    const [x, y, width, height] = execFileSync(x11Driver, ['geometry', title], { encoding: 'utf8' })
      .trim().split(' ').map(Number);

    const full = join(out, `${name}-root.png`);
    const cropped = join(out, `${name}.png`);
    execFileSync('magick', ['import', '-window', 'root', full]);
    execFileSync('magick', [full, '-crop', `${width}x${height}+${x}+${y}`, '+repage', cropped]);

    // isolated-run.sh binds FLECHA_VISUAL_OUT here; the scratch directory goes when the run passes.
    if (existsSync('/mnt/visual')) copyFileSync(cropped, join('/mnt/visual', `${name}.png`));

    const colors = histogram(cropped);
    assert(!colors.includes(foreign), `${name}: the ${title} window drew another partition's icon`);

    return colors.includes(expected);
  });

  const page = (mark, n) => `${base}/partition-icon?mark=${mark}&n=${n}`;

  await app.command('open', { url: page('r', 1), window: main });
  await shows('partition-regular', 'Flecha', main, partitionColors.r1, partitionColors.i1);

  const { window: incognito } = await openIncognito(app);

  await app.command('open', { url: page('i', 1), window: incognito });
  await shows('partition-incognito-after-regular', 'Flecha Incognito', incognito, partitionColors.i1, partitionColors.r1);
  pass('an incognito tab fetched its own copy of an icon the regular partition had downloaded');

  await app.command('open', { url: page('i', 2), window: incognito });
  await shows('partition-incognito', 'Flecha Incognito', incognito, partitionColors.i2, partitionColors.r2);
  await app.command('open', { url: page('r', 2), window: main });
  await shows('partition-regular-after-incognito', 'Flecha', main, partitionColors.r2, partitionColors.i2);
  pass('a regular tab fetched its own copy of an icon an incognito tab had downloaded');

  await app.command('new_space', { window: main });
  await app.command('isolate', { window: main });
  await app.command('open', { url: page('s', 1), window: main });
  await shows('partition-isolated-after-regular', 'Flecha', main, partitionColors.s1, partitionColors.r1);
  pass('a tab in an isolated Space fetched its own copy of an icon the regular partition had downloaded');
}

// Incognito windows: one in-memory cookie jar per session, kept apart from the regular one,
// gone when the last incognito window closes, and nowhere in the profile on disk afterwards.
function incognitoTab(snapshot, window) {
  const state = snapshot.incognito?.state;
  const record = state?.windows.find(entry => entry.id === window);
  assert(record, `no incognito window ${window}`);

  return record.spaces[record.activeSpaceId]?.activeTabId;
}

async function jarPage(app, window, path, jar, incognito) {
  const after = app.events.length;
  const url = `${base}${path}?jar=${jar}`;
  const snapshot = await app.command('open', { url, window });
  const tab = incognito ? incognitoTab(snapshot, window) : active(snapshot).activeTabId;

  const load = await app.wait(`HTTP load of ${path} for ${jar}`, event =>
    event.type === 'loaded' && event.tab === tab && event.url === url, after);

  assert.equal(load.status, 200, url);

  const titled = await app.wait(`${path} title for ${jar}`, event =>
    event.type === 'title' && event.tab === tab && event.title.startsWith(`${jar}:`), after);

  return { tab, title: titled.title };
}

async function openIncognito(app) {
  const after = app.events.length;
  const snapshot = await app.command('new_incognito_window');
  const window = snapshot.incognito.state.windows[0].id;
  await app.wait('GPUI to bring the incognito window to the front', event =>
    event.type === 'activated' && event.window === window, after);

  return { window, session: snapshot.incognito.session };
}

/** Asks `check` until it returns a value, within the per-step allowance. */
async function poll(label, check) {
  waitingFor = label;
  const deadline = Date.now() + stepTimeoutMs;

  for (;;) {
    const value = await check();

    if (value) return value;

    assert(Date.now() < deadline, `timed out waiting for ${label}`);
    await delay(100); // Matches the capture poll.
  }
}

/** Every file under `dir` whose bytes contain `needle`. */
function filesContaining(dir, needle) {
  const found = [];
  const bytes = Buffer.from(needle);

  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);

    if (entry.isDirectory()) found.push(...filesContaining(path, needle));
    else if (entry.isFile() && readFileSync(path).includes(bytes)) found.push(path);
  }

  return found;
}

async function incognitoChecks(app, main) {
  const run = Date.now().toString(36);
  const regularJar = `flecha_regular_${run}`;
  const privateJar = `flecha_incognito_${run}`;

  // Each jar page titles itself after reading its storage entry back, so a write that never
  // happened fails here instead of passing the disk scan.
  assert.equal((await jarPage(app, main, '/jar/set', regularJar, false)).title, `${regularJar}:set:stored`,
    'the regular page did not read back its storage entry');
  const first = await openIncognito(app);
  assert.equal(first.session, 1);

  assert.equal((await jarPage(app, first.window, '/jar/check', regularJar, true)).title, `${regularJar}:absent:empty`,
    'a regular cookie or storage entry reached the incognito window');
  const set = await jarPage(app, first.window, '/jar/set', privateJar, true);

  assert.equal(set.title, `${privateJar}:set:stored`, 'the incognito page did not read back its storage entry');
  assert.equal((await jarPage(app, first.window, '/jar/check', privateJar, true)).title, `${privateJar}:present:stored`,
    'the incognito window lost its own cookie or storage entry within the session');
  // The regular window lists its cookies without naming the incognito one in its address.
  const after = app.events.length;
  const listTab = active(await app.command('open', { url: base + '/jar/list', window: main })).activeTabId;
  await app.loaded(listTab, '/jar/list', after);

  const { title: listed } = await app.wait('regular cookie list', event =>
    event.type === 'title' && event.tab === listTab && event.title.startsWith('jars:'), after);

  assert(listed.includes(regularJar), `the regular window lost its cookie: ${listed}`);
  assert(!listed.includes(privateJar), 'an incognito cookie reached the regular window');
  pass('regular and incognito cookies and storage stayed apart, and the incognito ones lasted for their session');

  // A location grant in each kind of window. The incognito page is on another host name, so
  // its origin is one the regular window never visits.
  const privateOrigin = base.replace('127.0.0.1', 'localhost');
  const beforeGrant = app.events.length;
  const grantTab = incognitoTab(await app.command('open', { url: privateOrigin + '/open', window: first.window }), first.window);
  await app.wait('incognito page on localhost', event =>
    event.type === 'loaded' && event.tab === grantTab && event.url === privateOrigin + '/open', beforeGrant);
  assert.deepEqual(await app.command('allow_geolocation', { window: first.window }), { allowed: true });
  await app.command('select', { tab: listTab, window: main });
  assert.deepEqual(await app.command('allow_geolocation', { window: main }), { allowed: true });
  pass('Site Controls allowed location for an incognito page and a regular one');

  // A download from the incognito window saves its file and is listed in that window only.
  const privateDownload = row => row.url.includes(privateJar);
  await app.command('open', { url: `${base}/jar/download?jar=${privateJar}`, window: first.window });

  const downloaded = await poll('the incognito download to finish', async () => {
    const { downloads } = await app.command('snapshot');

    return downloads[first.window]?.some(row => privateDownload(row) && row.state === 'completed') && downloads;
  });

  const savedFile = join(process.env.FLECHA_PROFILE, 'downloads', `${privateJar}.txt`);
  assert.equal(readFileSync(savedFile, 'utf8'), downloadBody);
  assert(!downloaded[main].some(privateDownload), 'a regular window lists the incognito download');
  pass('a download from the incognito window saved its file and was listed in that window only');

  execFileSync(x11Driver, ['focus', 'Flecha Incognito']);

  const [, , , downloadWindowHeight] = execFileSync(x11Driver, ['geometry', 'Flecha Incognito'], { encoding: 'utf8' })
    .trim().split(' ').map(Number);

  await app.present(first.window);
  await captureUntil('incognito-downloads', 'Flecha Incognito', path => {
    // Inside the dark popover's left padding just above its 46px bottom inset.
    const color = execFileSync('magick', [path, '-format', `%[hex:p{12,${downloadWindowHeight - 60}}]`, 'info:'], { encoding: 'utf8' });

    return color.trim().toUpperCase().startsWith('161E2B') ? undefined : 'Downloads did not open automatically';
  });
  execFileSync(x11Driver, ['key', 'Escape']);
  pass('a new incognito download opened its Downloads popover');
  await app.command('select', { tab: grantTab, window: first.window });

  const running = await app.command('snapshot');
  const session = `incognito-${first.session}`;
  assert.deepEqual(running.browsers.find(page => page.tab === set.tab)?.partition, session);
  assert(running.contexts.includes(session), 'no incognito request context');
  assert(running.nativeWindows.includes(first.window), 'GPUI did not open the incognito window');
  assert(Object.values(running.core.tabs).every(tab => !tab.url.includes(privateJar)),
    'an incognito tab reached the regular state');
  await app.present(first.window);
  await capture('incognito', ['#323645', '#147A70'], 'Flecha Incognito');
  pass('the incognito window drew its violet sidebar beside the page');

  const closed = await app.command('close_window', { window: first.window });
  assert.equal(closed.incognito, null);
  assert(!closed.contexts.includes(session), 'the incognito context outlived its last window');
  await app.wait('CEF closure of the incognito pages', event => event.type === 'closed' && event.tab === set.tab);
  const second = await openIncognito(app);
  assert.equal(second.session, 2);
  assert.equal((await jarPage(app, second.window, '/jar/check', privateJar, true)).title, `${privateJar}:absent:empty`,
    'a new incognito session kept the last one\'s cookie or storage entry');
  const { downloads: afterSession } = await app.command('snapshot');
  assert(!Object.values(afterSession).flat().some(privateDownload), 'the ended session\'s download is still listed');
  assert.equal(readFileSync(savedFile, 'utf8'), downloadBody, 'closing the session removed the downloaded file');
  pass('closing the last incognito window destroyed its context: the next session started with an empty jar, and its download left the list but kept its file');

  // Restoring Spaces from before an import replaces the regular profile's pages only.
  const privatePage = incognitoTab(await app.command('snapshot'), second.window);

  const privateDocument = await document(app, privatePage, 'private-draft',
    "document.querySelector('input').value = 'private draft';");

  const privateBrowser = (await app.command('snapshot')).browsers.find(page => page.tab === privatePage)?.browser;
  assert(privateBrowser > 0, 'the incognito page has no browser');

  const privateKept = async step => {
    const { browsers } = await app.command('snapshot');
    assert.equal(browsers.find(page => page.tab === privatePage)?.browser, privateBrowser,
      `${step} replaced the incognito page's browser`);
    assert.deepEqual(await document(app, privatePage, `private-after-${step.replaceAll(/\W/g, '-')}`), privateDocument,
      `${step} lost the incognito page's document or draft`);
  };

  await restoreSpaces(app, listTab, '/jar/list');
  await privateKept('Restore Spaces');
  pass('Restore Spaces reloaded the regular pages and left the incognito page with its browser, document and draft');

  // Closing the last regular window leaves the incognito one open, as Chromium does.
  const closeRegular = async () => {
    await app.command('close_window', { window: main });

    return poll('GPUI to close the regular window', async () => {
      const snapshot = await app.command('snapshot');

      return !snapshot.nativeWindows.includes(main) && snapshot;
    });
  };

  const regularClosed = await closeRegular();

  assert.deepEqual(regularClosed.nativeWindows, [second.window], 'the incognito window closed with the regular one');
  assert(regularClosed.browsers.some(page => page.tab === privatePage), 'the incognito page closed with the regular window');
  assert(regularClosed.browsers.every(page => page.partition === `incognito-${second.session}`),
    `a regular page outlived its last window: ${JSON.stringify(regularClosed.browsers)}`);
  await privateKept('closing the regular window');

  // Ctrl+N in the incognito window, another launch, and another launch with an address each
  // bring the regular window back with its tabs, beside the incognito one.
  const reopened = async (step, after) => {
    const snapshot = await poll(`the regular window to come back after ${step}`, async () => {
      const state = await app.command('snapshot');

      return state.nativeWindows.includes(main) && state;
    });

    assert.deepEqual([...snapshot.nativeWindows].sort(), [main, second.window].sort(), `${step} opened another window`);
    assert.equal(snapshot.window, main);
    await app.wait(`the regular page to load again after ${step}`, event =>
      event.type === 'loaded' && event.tab === active(snapshot).activeTabId, after);
    await privateKept(step);

    return snapshot;
  };

  // A second launch of the binary hands its command line to the running Flecha and exits.
  const relaunch = args => execFileSync('./target/debug/flecha', args, {
    stdio: 'pipe',
    timeout: stepTimeoutMs,
    killSignal: 'SIGKILL',
  });

  execFileSync(x11Driver, ['focus', 'Flecha Incognito']);
  let reopenedAt = app.events.length;
  execFileSync(x11Driver, ['chord', 'Control_L', 'n']);
  assert.equal(active(await reopened('Ctrl+N', reopenedAt)).activeTabId, listTab, 'Ctrl+N lost the regular tab');
  await closeRegular();
  reopenedAt = app.events.length;
  relaunch([]);
  assert.equal(active(await reopened('a second launch', reopenedAt)).activeTabId, listTab, 'a second launch lost the regular tab');
  await closeRegular();
  pass('Ctrl+N and a second launch each brought the regular window back beside the incognito one');

  // A second launch's address is an external link: with "Open external links in Flechinha"
  // on, its default, it opens in a Flechinha on the regular Space and leaves the regular
  // window closed.
  await closeRegular();
  const relaunchUrl = `${base}/open?relaunched=${run}`;
  const linkedAt = app.events.length;
  relaunch(['--open', relaunchUrl]);

  const linked = await poll('the second launch\'s Flechinha', async () => {
    const state = await app.command('snapshot');

    return state.flechinhas.length === 1 && state.nativeWindows.includes(state.flechinhas[0].window) && state;
  });

  const [{ window: linkMini, tab: linkPage }] = linked.flechinhas;
  assert(!linked.nativeWindows.includes(main), 'the second launch\'s address reopened the regular window');
  await app.wait('the second launch\'s address in its Flechinha', event =>
    event.type === 'loaded' && event.tab === linkPage && event.url === relaunchUrl, linkedAt);
  const { browsers: linkBrowsers } = await app.command('snapshot');
  const linkBrowser = linkBrowsers.find(page => page.tab === linkPage);
  assert(linkBrowser?.owner?.kind === 'flechinha' && !String(linkBrowser.partition).startsWith('incognito-'),
    `the link's Flechinha left the regular request contexts: ${JSON.stringify(linkBrowsers)}`);
  await privateKept('a second launch with --open');
  await app.command('close_window', { window: linkMini });
  await poll('the link\'s Flechinha to close', async () => {
    const state = await app.command('snapshot');

    return !state.nativeWindows.includes(linkMini) && state;
  });
  pass('a second launch with --open showed its address in a Flechinha on the regular Space, beside the incognito window');

  // Closing the last window of either kind quits.
  const beforeQuit = app.events.length;

  await app.command('close_window', { window: second.window });

  const drained = await app.wait('every CEF browser to close', event => event.type === 'drained', beforeQuit);

  assert.equal(drained.live, 0, 'CEF browsers outlived the quit');
  await app.exited();
  pass('closing the last regular window left the incognito window and its page open, and closing that quit');

  const profile = process.env.FLECHA_PROFILE;
  const kept = filesContaining(profile, regularJar).map(path => relative(profile, path));
  assert(kept.length, 'the disk scan found nothing a regular window saved');
  assert.deepEqual(filesContaining(profile, privateJar), [], 'incognito browsing reached the profile on disk');
  assert(readFileSync(join(profile, 'permissions.json'), 'utf8').includes(base), 'the regular location grant was not saved');
  assert.deepEqual(filesContaining(profile, privateOrigin.slice('http://'.length)), [], 'the incognito location grant reached the disk');
  pass(`no file under the profile names the incognito cookie, storage entry, page, download or location grant; the regular jar is in ${kept.join(', ')}`);

  const restarted = launch('incognito-restart');
  await restarted.wait('restored CEF context ready', event => event.type === 'ready');
  const restored = await restarted.command('snapshot');
  assert.equal(restored.incognito, null);
  assert.deepEqual(restored.nativeWindows, [main], 'the regular window did not come back');
  assert(Object.values(restored.core.tabs).every(tab => !tab.url.includes(privateJar)));
  pass('restart restored no incognito window or tab');
  await restarted.quit();
}

// The checks for browsers pages open. Each reports its own failure, so a run
// against an older build shows every check it fails.
async function popupChecks(app) {
  const failed = [];

  const check = async (name, run) => {
    try {
      await run();
    } catch (error) {
      failed.push(name);
      console.error(`FAIL ${name} while waiting for ${waitingFor}`, error);
    }
  };

  await check('target=_blank', () => blankLink(app));
  await check('scripted popup', async () => scriptedPopup(app, await openTab(app, '/opener'), 'isolated'));
  await check('Flechinha links and popups', () => flechinhaLinks(app));
  await check('popup without a gesture', () => blockedPopups(app));
  await check('close with a popup open', () => closeWithPopup(app));
  assert.deepEqual(failed, [], 'popup checks failed');
}

async function blankLink(app) {
  const space = win(await app.command('new_space')).activeSpaceId;
  await app.command('isolate');
  const written = await openTab(app, '/cookie/set');
  const opener = await openTab(app, '/opener');
  const filler = await openTab(app, '/open');
  await app.command('select', { tab: opener });
  const requested = site.requests.length;
  const after = app.events.length;

  await clickElement(app, opener, '#blank', openerColors, 'blank-link');

  const { tab } = await app.wait('the target=_blank page load', event =>
    event.type === 'loaded' && event.url === base + '/cookie/check' && event.tab !== opener, after);

  const { title } = await app.wait('the new tab cookie result', event =>
    event.type === 'title' && event.tab === tab && ['Cookie present', 'Cookie absent', 'Shared cookie'].includes(event.title), after);

  const state = await app.command('snapshot');

  assert.equal(win(state).activeSpaceId, space, 'the link tab opened outside its Space');
  assert.deepEqual(active(state).today, [filler, opener, tab, written], 'the link tab is not right after its opener, as in the TS shell');
  assert.equal(active(state).activeTabId, tab, 'the link tab is not in the foreground');
  engine(state, tab, 'tab', state.window);
  assert.equal(title, 'Cookie present', 'the link tab used the wrong cookie jar');
  const request = site.requests.slice(requested).find(entry => entry.path === '/cookie/check');
  assert.equal(request?.referer, base + '/opener', 'the link tab lost the referrer Chromium sends');
  assert(app.events.slice(after).some(event =>
    event.type === 'opened' && event.tab === tab && event.disposition === 'foreground_tab'), 'CEF did not create the link tab');
  pass('a target=_blank click opened a foreground tab after its opener, in its Space and cookie jar, with its referrer');
}

// `flechinha` is the window of a Flechinha opener, which the sidebar can't select.
async function openPopup(app, opener, label, flechinha) {
  if (!flechinha) await app.command('select', { tab: opener });

  const after = app.events.length;
  const message = site.probe('popup-message');
  const hasOpener = site.probe('popup-opener');

  await clickElement(app, opener, '#scripted', openerColors, label, flechinha);

  const returned = await app.wait('window.open to return', event =>
    event.type === 'title' && event.tab === opener && event.title.startsWith('after-popup:'), after);

  assert(returned.title.endsWith(':true'), 'window.open returned null: the scripted popup was blocked');
  const opened = app.events.slice(after).find(event => event.type === 'opened' && event.disposition === 'popup');

  assert(opened, 'the popup has no CEF browser of its own');
  assert.deepEqual(opened.size, [480, 360], 'the popup lost its requested size');
  await app.loaded(opened.tab, '/popup', after);
  assert.equal(await hasOpener, 'true', 'the popup lost window.opener');
  assert.equal(await message, 'hello-from-popup', 'the opener did not receive the popup message');
  const state = await app.command('snapshot');
  const popup = state.popups.find(entry => entry.tab === opened.tab);

  assert.deepEqual(popup, { window: popup?.window, tab: opened.tab, opener: flechinha ?? state.window });
  assert(state.nativeWindows.includes(popup.window), 'GPUI did not open the popup window');
  engine(state, popup.tab, 'popup', popup.window);
  assert.equal(state.liveBrowsers, state.browsers.length, 'a CEF browser is alive outside the engine');
  await app.wait('the shell to bring the popup forward', event =>
    event.type === 'activated' && event.window === popup.window, after);

  return { ...popup, after, main: state.window };
}

async function scriptedPopup(app, opener, cookie) {
  const requested = site.requests.length;
  const popup = await openPopup(app, opener, 'scripted-popup');
  const request = site.requests.slice(requested).find(entry => entry.path === '/popup');

  assert.equal(request.cookie, cookie, 'the popup used the wrong cookie jar');
  assert.equal(request.referer, base + '/opener', 'the popup lost its referrer');
  const size = site.probe('popup-size');
  await app.command('script', { tab: popup.tab, source: "fetch('/probe/popup-size?value=' + innerWidth + 'x' + innerHeight)" });
  assert.equal(await size, '480x360', 'the popup page is not the size it asked for');
  pass('a scripted popup opened in its own window with window.opener, postMessage, its size and its opener cookie jar');

  await app.wait('the popup title', event =>
    event.type === 'title' && event.tab === popup.tab && event.title === 'Flecha fixture /popup', popup.after);
  await app.present(popup.window);
  execFileSync(x11Driver, ['focus', 'Flecha fixture /popup']);
  await keyDelivery(app, { O: opener, P: popup.tab }, 'k', 'popup', 'P');
  // A popup has no command bar for Ctrl+L to focus.
  execFileSync(x11Driver, ['chord', 'Control_L', 'l']);
  await keyDelivery(app, { O: opener, P: popup.tab }, 'm', 'popup after Ctrl+L', 'P');

  const closing = app.events.length;
  await app.command('script', { tab: popup.tab, source: 'window.close()' });
  await app.wait('the popup beforeunload dialog', event =>
    event.type === 'dialog_changed' && event.tab === popup.tab, closing, stepTimeoutMs);
  await app.present(popup.window);
  await app.command('key', { window: popup.window, keystroke: 'enter' });
  await app.wait('the popup to close itself', event => event.type === 'closed_by_page' && event.tab === popup.tab, closing);
  const closed = await app.command('snapshot');

  assert.deepEqual(closed.popups, [], 'the shell kept the closed popup');
  assert(!closed.nativeWindows.includes(popup.window), 'GPUI kept the closed popup window');
  assert(!closed.browsers.some(browser => browser.tab === popup.tab), 'the engine kept the closed popup browser');
  assert.equal(closed.liveBrowsers, closed.browsers.length, 'a CEF browser is alive outside the engine');
  await app.wait('focus to return to the opener window', event =>
    event.type === 'activated' && event.window === popup.main, closing);
  execFileSync(x11Driver, ['focus', 'Flecha']);
  await keyDelivery(app, { O: opener }, 'j', 'popup closed', 'O');
  pass('the popup closed itself, and focus and keys returned to its opener');
}

async function flechinhaLinks(app) {
  // The main window shows this page once the opener leaves, not another opener.
  await openTab(app, '/open');
  await openTab(app, '/opener');
  const [{ window: mini, tab: page }] = (await app.command('detach')).flechinhas;
  const after = app.events.length;

  await clickElement(app, page, '#blank', openerColors, 'flechinha-blank', mini);

  const { tab } = await app.wait("a Flechinha link's tab load", event =>
    event.type === 'loaded' && event.url === base + '/cookie/check' && event.tab !== page, after);

  const state = await app.command('snapshot');

  engine(state, tab, 'tab', state.window);
  assert.equal(active(state).today[0], tab, "the Flechinha link tab is not first in the main window's Today");
  assert.equal(active(state).activeTabId, tab, 'the Flechinha link tab is not in the foreground');
  pass('a target=_blank link in a Flechinha opened a foreground tab in the main window, as patch 0023 does');

  const popup = await openPopup(app, page, 'flechinha-popup', mini);
  const closing = app.events.length;

  await app.command('script', { tab: popup.tab, source: 'window.close()' });

  const result = await app.wait('the Flechinha popup to close or ask before leaving', event =>
    ['dialog_changed', 'closed_by_page'].includes(event.type) && event.tab === popup.tab, closing, stepTimeoutMs);

  if (result.type === 'dialog_changed') {
    await app.present(popup.window);
    await app.command('key', { window: popup.window, keystroke: 'enter' });
  }

  await app.wait('the Flechinha popup to close itself', event =>
    event.type === 'closed_by_page' && event.tab === popup.tab, closing);
  await app.wait('focus to return to the Flechinha', event => event.type === 'activated' && event.window === mini, closing);
  await app.command('close_window', { window: mini });
  pass('a scripted popup from a Flechinha opened with its opener, closed itself and returned focus to the Flechinha');
}

async function blockedPopups(app) {
  const requested = site.requests.length;
  const after = app.events.length;
  const tab = active(await app.command('open', { url: base + '/opener?auto=1' })).activeTabId;

  const { title } = await app.wait('window.open without a gesture', event =>
    event.type === 'title' && event.tab === tab && event.title.startsWith('after-blocked:'), after);

  assert.equal(title, 'after-blocked:true:true', 'a popup without a user gesture opened');
  assert.deepEqual(app.events.slice(after).filter(event => event.type === 'opened'), [], 'CEF created a browser without a gesture');
  assert.deepEqual(site.requests.slice(requested).filter(entry => entry.path.startsWith('/blocked-')), [],
    'a blocked popup reached the network');
  const state = await app.command('snapshot');

  assert.deepEqual(state.popups, []);
  assert.equal(state.liveBrowsers, state.browsers.length, 'a CEF browser is alive outside the engine');
  pass('window.open without a user gesture was blocked for a popup and a tab, as in the TS shell');
}

async function closeWithPopup(app) {
  const opener = await openTab(app, '/opener');
  const first = await openPopup(app, opener, 'close-button-popup');
  let after = app.events.length;

  await app.command('close_window', { window: first.window });
  await app.wait('CEF to close the popup', event => event.type === 'closed' && event.tab === first.tab, after);
  const closed = await app.command('snapshot');

  assert.deepEqual(closed.popups, []);
  assert(!closed.nativeWindows.includes(first.window), 'GPUI kept the closed popup window');
  assert.equal(closed.liveBrowsers, closed.browsers.length, 'a CEF browser is alive outside the engine');
  pass("the popup window's close button closed its CEF browser");

  after = app.events.length;
  const { window: extra } = await app.command('open_window', { url: base + '/split' });
  const opened = (await app.command('snapshot')).core.windows.find(window => window.id === extra);
  const tab = opened.spaces[opened.activeSpaceId].activeTabId;

  await app.loaded(tab, '/split', after);
  const shown = await app.command('snapshot');

  engine(shown, tab, 'tab', extra);
  assert(shown.nativeWindows.includes(extra), 'GPUI did not open the second Flecha window');
  after = app.events.length;
  await app.command('close_window', { window: extra });
  await app.wait("CEF to close the closed window's tab", event => event.type === 'closed' && event.tab === tab, after);
  const gone = await app.command('snapshot');

  assert(!gone.core.windows.some(window => window.id === extra), 'the closed window is still open');
  assert.equal(gone.core.closedWindows[0]?.id, extra, 'the closed window cannot be reopened');
  assert(!gone.nativeWindows.includes(extra), 'GPUI kept the closed window');
  assert.equal(gone.liveBrowsers, gone.browsers.length, 'a CEF browser is alive outside the engine');
  pass('closing one of two Flecha windows closed its CEF browsers and kept it with the closed windows, without quitting');

  // A page with a beforeunload handler and a user gesture must not hold up the quit.
  const unload = await openTab(app, '/unload');
  await clickElement(app, unload, 'input', ['#147A70', '#FFCC49'], 'unload-gesture');
  const second = await openPopup(app, opener, 'quit-popup');
  const open = await app.command('snapshot');

  assert(open.browsers.some(browser => browser.tab === unload) && open.browsers.some(browser => browser.tab === second.tab));
  after = app.events.length;
  await app.command('close_window', { window: second.main });
  const drained = await app.wait('every CEF browser to close', event => event.type === 'drained', after);

  assert.equal(drained.live, 0, 'CEF browsers outlived the quit');
  await app.exited();
  pass(`closing the last Flecha window with a popup and a beforeunload page open closed all ${open.liveBrowsers} CEF browsers and quit`);
}

/**
 * Captures each chrome surface in Light and Dark for the visual parity pass: the sidebar with
 * pinned, folder, favorite and Today tabs, then each overlay over it. Captures land beside the
 * run's logs as `visual-<surface>-<theme>.png`, cropped to the window.
 */
async function visualParity(app, left) {
  for (const path of ['/split', '/peek', '/opener']) {
    const opened = await app.command('open', { url: base + path });
    await app.loaded(active(opened).activeTabId, path);
  }

  let state = await app.command('new_folder', { name: 'Reading' });
  const space = win(state).activeSpaceId;
  const records = state.core.spaces.find(candidate => candidate.id === space).pinnedFlat;
  const folder = records.find(record => record.name === 'Reading').id;
  const [newest, second, third] = active(state).today;
  await app.command('pin', { tab: third, folder });
  await app.command('pin', { tab: second });
  await app.command('favorite', { tab: left });
  await app.command('new_space');
  await app.command('switch', { space });
  state = await app.command('select', { tab: newest });
  assert.equal(active(state).activeTabId, newest);

  const shot = async (name, window = 'Flecha') => {
    await app.present();
    // GPUI's animations (popovers rise in --medium, 200ms) settle before the capture.
    await delay(400);
    await app.present();

    const [x, y, width, height] = execFileSync(x11Driver, ['geometry', window], { encoding: 'utf8' })
      .trim().split(' ').map(Number);

    const full = join(out, `${name}-root.png`);
    execFileSync('magick', ['import', '-window', 'root', full]);
    const cropped = join(out, `${name}.png`);
    execFileSync('magick', [full, '-crop', `${width}x${height}+${x}+${y}`, '+repage', cropped]);

    // isolated-run.sh binds FLECHA_VISUAL_OUT here; the scratch directory goes when the run passes.
    if (existsSync('/mnt/visual')) copyFileSync(cropped, join('/mnt/visual', `${name}.png`));
  };

  const geometry = () => execFileSync(x11Driver, ['geometry', 'Flecha'], { encoding: 'utf8' })
    .trim().split(' ').map(Number);

  const key = (...keys) => execFileSync(x11Driver, keys.length > 1 ? ['chord', ...keys] : ['key', keys[0]]);
  const click = (x, y) => execFileSync(x11Driver, ['click', Math.round(x), Math.round(y)]);

  for (const theme of ['light', 'dark']) {
    await app.command('theme', { theme });
    execFileSync(x11Driver, ['focus', 'Flecha']);
    const [x, y, width, height] = geometry();
    // Away from the sidebar's controls, so no row shows its hover state.
    click(x + width - 40, y + height - 40);
    await shot(`visual-sidebar-${theme}`);

    key('Control_L', 'l');
    await shot(`visual-command-bar-${theme}`);
    key('Escape');

    key('Control_L', 'comma');
    await shot(`visual-settings-${theme}`);
    key('Escape');

    key('Control_L', 'h');
    await shot(`visual-history-${theme}`);
    key('Escape');

    key('Control_L', 'f');
    await shot(`visual-find-${theme}`);
    key('Escape');

    key('Control_L', 'equal');
    await shot(`visual-zoom-${theme}`);
    key('Control_L', '0');

    // Near the page's left edge, so the menu opens to the right without clipping.
    execFileSync(x11Driver, ['right-click', x + 260 + 120, y + 200]);
    await shot(`visual-context-menu-${theme}`);
    key('Escape');

    click(x + menuButton[0], y + menuButton[1]);
    await shot(`visual-app-menu-${theme}`);
    // The Zoom row opens its submenu beside it.
    click(x + zoomRow[0], y + zoomRow[1]);
    await shot(`visual-app-menu-zoom-${theme}`);
    key('Escape');

    execFileSync(x11Driver, ['right-click', x + spaceHeader[0], y + spaceHeader[1]]);
    await shot(`visual-space-menu-${theme}`);
    key('Escape');

    // The footer's Downloads button: the sidebar's left and bottom padding, then half the
    // 28px icon button (ui/styles.css:32, 136).
    click(x + 10 + 14, y + height - 8 - 14);
    await shot(`visual-downloads-${theme}`);
    key('Escape');

    // The address's site controls button: right of the 36px address, inside its 4px padding
    // and the sidebar's 6px (ui/styles.css:32, 177-178, 203).
    click(x + 260 - 6 - 4 - 13, y + 10 + 28 + 8 + 18);
    await shot(`visual-site-controls-${theme}`);
    key('Escape');

    await app.command('peek', { url: base + '/peek' });
    await shot(`visual-peek-${theme}`);
    await app.command('close_peek');

    await app.command('split', { url: base + '/split' });
    await shot(`visual-split-${theme}`);
    await app.command('close');
    await app.command('select', { tab: newest });

    const [{ window: mini }] = (await app.command('detach')).flechinhas;
    await shot(`visual-flechinha-${theme}`, 'Flechinha');
    const returned = await app.command('return', { window: mini });
    await app.command('select', { tab: active(returned).activeTabId });
  }

  // The window menu's rows act: New window opens a regular window.
  execFileSync(x11Driver, ['focus', 'Flecha']);
  const [x, y] = geometry();
  const windows = async () => (await app.command('snapshot')).core.windows;
  const before = await windows();

  click(x + menuButton[0], y + menuButton[1]);
  await app.present();
  click(x + newWindowRow[0], y + newWindowRow[1]);

  const opened = (await poll('New window from the window menu', async () => {
    const now = await windows();

    return now.length === before.length + 1 && now;
  })).find(window => !before.some(old => old.id === window.id));

  await app.command('close_window', { window: opened.id });
  await poll('GPUI closed the menu\'s window', async () =>
    !(await app.command('snapshot')).nativeWindows.includes(opened.id));
  pass('the window menu opened a new window');

  // The Space menu, from a right-click on the Space's name (background.ts spaceMenu), turns
  // Separate Cookies and Logins on and off. The driver focuses a title only one window has.
  await poll('one window named Flecha', async () => {
    try {
      execFileSync(x11Driver, ['focus', 'Flecha'], { stdio: 'ignore' });

      return true;
    } catch {
      return false;
    }
  });

  const shown = async () => {
    const snapshot = await app.command('snapshot');

    return snapshot.core.spaces.find(space => space.id === win(snapshot).activeSpaceId);
  };

  for (const wanted of [true, false]) {
    // GPUI hit-tests the last frame drawn, which may still hold the previous menu.
    await app.present();
    execFileSync(x11Driver, ['right-click', x + spaceHeader[0], y + spaceHeader[1]]);
    await app.present();
    click(x + spaceHeader[0] + spaceMenuItem[0], y + spaceHeader[1] + spaceMenuItem[1]);
    await poll(`Separate Cookies and Logins ${wanted ? 'on' : 'off'} from the Space menu`, async () =>
      (await shown()).isolated === wanted);
  }

  pass('the Space menu turned Separate Cookies and Logins on and off');

  const { window: incognito } = await openIncognito(app);
  await app.command('open', { url: base + '/open', window: incognito });
  await shot('visual-incognito-dark', 'Flecha Incognito');
  await app.command('close_window', { window: incognito });
  pass('captured each chrome surface in Light and Dark');
}

const apps = [];

function launch(name, fixturePort) {
  const app = browser(name, fixturePort);
  apps.push(app);

  return app;
}

async function requestFilterResult(app, tab, label) {
  const result = site.probe(label);
  await app.command('script', {
    tab,
    source: `Promise.all(['/fixture-ad', '/fixture-tracker'].map(path =>
      fetch(path, { cache: 'no-store' }).then(response => response.ok, () => false)
    )).then(results => fetch('/probe/${label}?value=' + JSON.stringify(results)));`,
  });
  waitingFor = label;

  return JSON.parse(await result);
}

async function verifyAdblock() {
  const cache = join(process.env.FLECHA_PROFILE, 'adblock');
  const listFile = join(cache, 'easylist.txt');
  assert(!existsSync(cache), 'the scratch profile already has ad-block lists');

  const port = site.server.address().port;
  const first = launch('adblock-first', port);
  const initialFailure = await first.wait('failed initial list downloads', event => event.type === 'adblock' && event.failed);
  assert.deepEqual(listRequests, new Set(['/easylist.txt', '/easyprivacy.txt']));
  assert(!existsSync(cache), 'the failed downloads created a cache');
  await first.wait('CEF context ready', event => event.type === 'ready');
  const tab = await openTab(first, '/open');
  await first.present();
  assert.deepEqual(await requestFilterResult(first, tab, 'before-lists-arrive'), [true, true]);
  listResponse = 'valid';
  pass('started with no list cache and displayed a page when both list downloads failed');

  const retryBudget = 2 * initialFailure.downloadTimeoutMs + stepTimeoutMs;
  await first.wait('automatic list retry', event => event.type === 'adblock' && !event.failed, 0, retryBudget);
  assert.deepEqual(await requestFilterResult(first, tab, 'fresh-list-filter'), [false, false]);
  assert.equal(readFileSync(listFile, 'utf8'), easylist);
  assert.equal(readFileSync(join(cache, 'easyprivacy.txt'), 'utf8'), easyprivacy);
  await first.quit();
  pass('retried automatically, cached both lists, and blocked requests in the already-open tab');

  // Make EasyList due using the filesystem clock the updater reads.
  utimesSync(listFile, 0, 0);
  listResponse = 'corrupt';
  listRequests.clear();
  const restarted = launch('adblock-corrupt-refresh', port);
  await restarted.wait('fall back to the cached list', event => event.type === 'adblock' && event.usedCache);
  assert.deepEqual(listRequests, new Set(['/easylist.txt']));
  assert.equal(readFileSync(listFile, 'utf8'), easylist);
  await restarted.wait('CEF context ready after the corrupt refresh', event => event.type === 'ready');
  const restored = await openTab(restarted, '/split');
  assert.deepEqual(await requestFilterResult(restarted, restored, 'cached-list-filter'), [false, false]);
  await restarted.quit();
  pass('rejected a bad EasyList checksum and kept the previous list and its blocking decisions');

  listRequests.clear();
  const cached = launch('adblock-cache-only');
  await cached.wait('load cached lists without a fixture port', event => event.type === 'adblock' && !event.failed, 0, stepTimeoutMs);
  await cached.wait('CEF context ready from the cache-only launch', event => event.type === 'ready');
  const loaded = await openTab(cached, '/open');
  assert.deepEqual(await requestFilterResult(cached, loaded, 'cache-only-filter'), [false, false]);
  assert.equal(listRequests.size, 0);
  await cached.quit();
  pass('loaded cached lists without a fixture port or network requests');

  for (const invalidPort of ['bad-port', '0']) {
    assert.throws(() => execFileSync('./target/debug/flecha', ['--verify'], {
      env: { ...process.env, FLECHA_VERIFY_ADBLOCK_PORT: invalidPort },
      stdio: 'pipe',
      timeout: stepTimeoutMs,
      killSignal: 'SIGKILL',
    }), error => error.status === 1 && error.stderr.includes('FLECHA_VERIFY_ADBLOCK_PORT must be a nonzero TCP port'));
  }

  pass('rejected malformed and zero fixture ports at startup');
}

async function verifySlice() {
  const app = launch('first');
  await app.wait('CEF context ready', event => event.type === 'ready');
  const opened = await app.command('open', { url: base + '/open' });
  const main = opened.window;
  const shared = win(opened).activeSpaceId;
  const left = active(opened).activeTabId;
  const names = new Map([[left, 'L']]);
  compareShipping('open', opened, names);
  await app.loaded(left, '/open');
  await app.wait('CEF paint for initial page', event => event.type === 'paint' && event.tab === left);
  const checked = checkProcessEnvironment(app.pid);
  pass(`checked /proc environ for the app and its CEF descendants (${checked} processes, including a renderer or zygote child)`);
  await capture('open', ['#147A70', '#FFCC49']);
  pass('opened a durable tab, received CEF load and paint, and captured the GPUI fixture frame');
  // Under Xvfb GPUI's adapter cannot import Vulkan dma-bufs, so these checks cover the CPU
  // copy path, not shared textures.
  const frames = /^page frames: .*$/m.exec(readFileSync(join(out, 'first.log'), 'utf8'))?.[0] ?? '';
  assert.match(frames, /^page frames: CPU copies, shared textures are unavailable: /, 'Xvfb run did not use CPU copies');
  pass(`Xvfb pages use CPU copies (${frames})`);
  await nativeInput(app, left);

  if (mode === 'slice') {
    await nativeContextMenu(app);
    await tabRowInput(app, left);
  }

  if (mode === 'focus') {
    await sidebarFocus(app, left);
    await restoreSpaces(app, left);
    await scriptedPopup(app, await openTab(app, '/opener'), undefined);
    await app.quit();
  } else if (mode === 'popups') {
    await popupChecks(app);
  } else if (mode === 'incognito') {
    await incognitoChecks(app, main);
  } else if (mode === 'fullscreen') {
    await pageFullscreen(app, main, left);
  } else if (mode === 'tab-status') {
    await tabStatus(app, main, left);
    await partitionIcons(app, main);
    await app.quit();
  } else if (mode === 'visual') {
    await visualParity(app, left);
    await app.quit();
  } else if (mode !== 'open') {
    const publicUrl = base.replace('127.0.0.1', 'localtest.me') + '/open';
    const attempted = await app.command('peek', { url: publicUrl });
    const [{ tab: publicTab }] = attempted.peeks;

    const navigation = await app.wait('isolated DNS result', event =>
      ['load_failed', 'loaded'].includes(event.type) && event.tab === publicTab && event.url === publicUrl);

    assert.equal(navigation.type, 'load_failed', 'CEF loaded the loopback fixture through the public name');
    await app.command('close_peek');
    pass(`CEF rejected the localtest.me navigation (net error ${navigation.code})`);

    const split = await app.command('split', { url: base + '/split' });
    const [, right] = active(split).split;
    names.set(right, 'R');
    compareShipping('split', split, names);
    await app.loaded(right, '/split');
    await app.wait('paint of the right split pane', event => event.type === 'paint' && event.tab === right);
    await splitCapture(app, 'split', split);
    const running = await app.command('snapshot');
    engine(running, left, 'tab', main);
    engine(running, right, 'tab', main);
    pass('split the selected tab, loaded the second pane, and captured both pages at once');

    const peek = await app.command('peek', { url: base + '/peek' });
    const [{ tab: peekTab }] = peek.peeks;
    assert.deepEqual(peek.core, running.core);
    await app.loaded(peekTab, '/peek');
    await capture('peek', ['#235089', '#83DDFF']);
    const peekBrowser = engine(await app.command('snapshot'), peekTab, 'peek', main);
    const peekDocument = await document(app, peekTab, 'peek', "document.querySelector('input').value = 'unfinished draft';");
    assert(peekDocument.token);
    assert.equal(peekDocument.draft, 'unfinished draft');
    const promoted = await app.command('promote');
    assert.deepEqual(promoted.peeks, []);
    names.set(peekTab, 'P');
    compareShipping('promote', promoted, names);
    assert.equal(engine(promoted, peekTab, 'tab', main), peekBrowser);
    assert.deepEqual(await document(app, peekTab, 'promoted'), peekDocument);
    pass('promoted the transient Peek into Today with its browser, document and draft intact');

    const detached = await app.command('detach');
    compareShipping('detach', detached, names);
    const [{ window: mini, tab: moved }] = detached.flechinhas;
    assert(!detached.core.tabs[peekTab]);
    assert(!detached.core.windows.some(window => window.id === mini));
    assert.equal(engine(detached, moved, 'flechinha', mini), peekBrowser);
    assert.deepEqual(await document(app, moved, 'detached'), peekDocument);
    await capture('flechinha', ['#235089', '#83DDFF'], 'Flechinha');
    assert((await app.command('snapshot')).nativeWindows.includes(mini), 'GPUI opened the Flechinha window');
    const returned = await app.command('return', { window: mini });
    assert.deepEqual(returned.flechinhas, []);
    names.set(moved, 'M');
    compareShipping('return', returned, names);
    assert.equal(engine(returned, moved, 'tab', main), peekBrowser);
    assert.deepEqual(await document(app, moved, 'returned'), peekDocument);
    assert(!(await app.command('snapshot')).nativeWindows.includes(mini), 'GPUI closed the emptied Flechinha window');
    pass('moved the live document to Flechinha and back to the last active Flecha window');

    const closeUrl = base + '/closed';
    const closeTitle = 'Flecha fixture /closed';
    assert(returned.core.archive.every(entry => entry.url !== closeUrl && entry.title !== closeTitle));
    const beforeNavigation = app.events.length;
    await app.command('script', { tab: moved, source: "location.assign('/closed')" });
    await app.loaded(moved, '/closed', beforeNavigation);
    await app.wait('unique title for closing tab', event =>
      event.type === 'title' && event.tab === moved && event.title === closeTitle, beforeNavigation);

    while (Date.now() <= returned.core.tabs[moved].lastActive) await delay(0);
    const closeStartedAt = Date.now();
    await app.command('close');
    await app.wait('CEF closure of returned tab', event => event.type === 'closed_by_page' && event.tab === moved);
    const closed = await app.command('snapshot');
    assert(!closed.core.tabs[moved]);
    assert.deepEqual(closed.core.archive.slice(1), returned.core.archive, 'closing did not add a fresh archive entry');
    const entry = closed.core.archive[0];
    assert.deepEqual([entry.url, entry.title], [closeUrl, closeTitle], 'new archive entry belongs to a different page');
    assert(returned.core.archive.every(old => entry.closedAt > old.closedAt), 'new archive entry does not have a later close time');
    assert(entry.closedAt >= closeStartedAt, 'archive timestamp predates the close command');
    compareShipping('close', closed, names);
    pass('closed the returned tab, added a new archive entry, and matched the shipping TS trace');

    const after = app.events.length;
    const selected = await app.command('select', { tab: left });
    assert.equal(active(selected).activeTabId, left);
    const newest = await app.command('recreate', { tab: left });
    await app.loaded(left, '/open', after);
    await app.wait('paint from the replacement browser', event =>
      event.type === 'paint' && event.tab === left && event.generation === newest.current, after);
    const delivered = app.events.slice(after).filter(event => event.tab === left);
    assert.deepEqual(delivered.filter(event => event.generation === newest.retired).map(event => event.type),
      [], 'stale CEF generation reached the shell');
    assert.deepEqual(delivered.filter(event => event.type === 'created').map(event => event.browser), [newest.browser]);
    pass('dropped retired Created, Paint, Title and Address callbacks before shell dispatch');

    const sharedWrite = await app.command('open', { url: base + '/cookie/set-shared' });
    assert.deepEqual(active(sharedWrite).today, [active(sharedWrite).activeTabId, ...active(closed).today],
      'foreground open did not prepend Today');
    await app.loaded(active(sharedWrite).activeTabId, '/cookie/set-shared');
    await cookie(app, 'Shared cookie');
    const isolated = win(await app.command('new_space')).activeSpaceId;
    const isolatedState = await isolateWithTransients(app, isolated);
    assert.deepEqual(isolatedState.core.windows[0].spaces[isolated]?.today ?? [], []);
    await cookie(app, 'Cookie absent', 'peek');
    await app.command('close_peek');
    pass('Peek in an empty isolated Space did not receive the shared cookie');
    const written = await app.command('open', { url: base + '/cookie/set' });
    await app.loaded(active(written).activeTabId, '/cookie/set');
    await cookie(app, 'Cookie present');
    await app.command('switch', { space: shared });
    await cookie(app, 'Shared cookie');
    await spaceNumberKeys(app, [shared, isolated]);
    const before = await app.command('select', { tab: left });
    pass('two tabs in the isolated Space shared the cookie and the ordinary Space did not receive it');
    await app.quit();

    if (mode === 'break-restore') {
      const saved = join(process.env.FLECHA_PROFILE, 'session', 'state.json');
      renameSync(saved, saved + '.negative-control');
    }

    const restarted = launch('restart');
    await restarted.wait('restored CEF context ready', event => event.type === 'ready');
    const restored = await restarted.command('snapshot');
    assert.equal(restored.window, main, 'restore lost the persisted window');
    assert.deepEqual(restored.core, before.core, 'restore changed durable state');
    assert.deepEqual(active(restored).split, [left, right]);
    assert.deepEqual(restored.peeks, []);
    assert.deepEqual(restored.flechinhas, []);
    await restarted.loaded(left, '/open');
    await restarted.loaded(right, '/split');

    for (const tab of [left, right]) await restarted.wait('paint from the restored split', event =>
      event.type === 'paint' && event.tab === tab);
    await splitCapture(restarted, 'restart-split', restored);
    pass('restarted on the same scratch profile and captured both restored pages in their original panes');

    await restarted.command('switch', { space: isolated });
    await cookie(restarted, 'Cookie present');
    await restarted.command('switch', { space: shared });
    await cookie(restarted, 'Shared cookie');
    pass('the isolated cookie persisted after restart and stayed out of the ordinary Space');

    if (mode === 'slice') {
      await quickWins(restarted);
      await restarted.command('switch', { space: shared });
    }

    const dragged = mode === 'slice' ? await tabDrags(restarted) : undefined;

    await restarted.quit();

    const popups = launch('popups');
    await popups.wait('CEF context ready for popups', event => event.type === 'ready');

    if (dragged) {
      assert.deepEqual((await popups.command('snapshot')).core, dragged);
      pass('tab drag order, placements and split survived an isolated restart');
      await popups.command('switch', { space: shared });
      const tab = await openTab(popups, '/browser-tools');
      execFileSync(x11Driver, ['focus', 'Flecha']);
      await sidebarResizeAndReveal(popups, tab);
      await browserDialogs(popups);
      await popups.command('select', { tab });
      await popups.present();
      const beforePrint = popups.events.length;
      execFileSync(x11Driver, ['chord', 'Control_L', 'p']);

      const printed = await popups.wait('CEF to render a PDF for the print dialog', event =>
        event.type === 'print_ready' && event.tab === tab, beforePrint, stepTimeoutMs);

      assert.equal(printed.pdf, true);
      pass('Ctrl+P rendered the active page to a PDF for the Linux print portal');
      execFileSync(x11Driver, ['chord', 'Control_L', 'u']);

      const source = await poll('View Source to open a new tab', async () => {
        const state = await popups.command('snapshot');
        const selected = active(state).activeTabId;

        return state.core.tabs[selected]?.url === 'view-source:' + base + '/browser-tools' && selected;
      });

      assert.notEqual(source, tab);
      await popups.wait('the source tab to display its page', event =>
        event.type === 'loaded' && event.tab === source && event.status === 200, 0, stepTimeoutMs);
      pass('Ctrl+U opened and loaded a view-source tab for the active page');
      await rejectInvalidCertificate(popups);
    }

    await popupChecks(popups);
  } else {
    await app.quit();
  }

  pass('CEF shut down cleanly');
}

/**
 * Copy, Cut and Paste between web pages and GPUI's clipboard, from the keyboard and the page
 * menu. Under X11 the keys go through XTest; a display without input injection dispatches them
 * to the GPUI window. Every string is a fixture value.
 */
async function verifyClipboard() {
  // Flecha prefers Wayland whenever WAYLAND_DISPLAY is set, even with an XWayland DISPLAY.
  const x11 = !process.env.WAYLAND_DISPLAY;
  const app = launch('clipboard');
  await app.wait('CEF context ready', event => event.type === 'ready');
  const opened = await app.command('open', { url: base + '/clipboard' });
  const tab = active(opened).activeTabId;
  await app.loaded(tab, '/clipboard');
  await app.wait('CEF paint for the clipboard page', event => event.type === 'paint' && event.tab === tab);
  await app.present();

  if (x11) {
    execFileSync('cc', [
      '-std=c17', '-Wall', '-Wextra', '-Werror', 'spikes/gpui-cef-compositor/x11-input.c',
      '-lX11', '-lXtst', '-o', x11Driver,
    ]);
    execFileSync(x11Driver, ['focus', 'Flecha']);
  }

  const backend = x11 ? 'X11' : 'Wayland';

  // Each chord is X11 key names, sent back to back with no wait between them under X11.
  const keys = async (...chords) => {
    for (const chord of chords) {
      if (x11) {
        execFileSync(x11Driver, chord.length === 1 ? ['key', ...chord] : ['chord', ...chord]);
      } else {
        const names = { Control_L: 'ctrl', Shift_L: 'shift' };

        await app.command('key', { keystroke: chord.map(name => names[name] ?? name.toLowerCase()).join('-') });
      }
    }
  };

  const press = key => keys(['Control_L', key]);

  let probes = 0;

  const page = async (step, source = 'null', target = tab) => {
    const label = `clipboard-${++probes}`;
    const reply = site.probe(label, stepTimeoutMs);
    reply.catch(() => {}); // A failed command rejects first; the probe's timeout must not go unhandled.
    await app.command('script', {
      tab: target,
      source: `{
        const field = selector => document.querySelector(selector);
        const result = (() => { ${step}; return ${source}; })();
        fetch('/probe/${label}?value=' + encodeURIComponent(JSON.stringify(result)), { cache: 'no-store' });
      }`,
    });
    waitingFor = `clipboard page step ${label}`;

    return JSON.parse(await reply);
  };

  const clipboard = async text => (await app.command('clipboard', text === undefined ? {} : { text })).text;
  const sentinel = 'flecha-clipboard-sentinel';

  const center = selector => page('', `(() => {
    const box = field('${selector}').getBoundingClientRect();
    return [box.x + box.width / 2, box.y + box.height / 2];
  })()`);

  // The cross-site frame's own report of its fields, focus and selection.
  const frameState = async () => {
    const label = `frame-${++probes}`;
    const reply = site.probe(label, stepTimeoutMs);
    reply.catch(() => {}); // A failed command rejects first; the probe's timeout must not go unhandled.
    await page(`field('#frame').contentWindow.postMessage(['report', '${label}'], '*')`);
    waitingFor = `the frame's ${label} report`;

    return JSON.parse(await reply);
  };

  // Focuses the frame, asks it to run `command`, and waits until its report passes `ready`.
  const frame = async (command, ready) => {
    await page(`field('#frame').focus(); field('#frame').contentWindow.postMessage(['${command}'], '*')`);

    return poll(`the frame to ${command}`, async () => {
      const state = await frameState();

      return ready(state) ? state : undefined;
    });
  };

  // Wayland's handling everywhere, since headless weston gives GPUI no input serial.
  await app.command('shell_clipboard');

  assert.equal(await clipboard(sentinel), sentinel, `GPUI's ${backend} clipboard did not keep a fixture string`);

  // Each target starts empty, focused and with its caret at the end.
  const targets = [
    ['#text', 'value', 'flecha-paste-text'],
    ['#password', 'value', 'flecha-paste-secret'],
    ['#area', 'value', 'flecha-paste line one\nline two'],
    ['#editable', 'textContent', 'flecha-paste-editable'],
  ];

  for (const [selector, property, text] of targets) {
    await clipboard(text);
    await page(`field('${selector}').${property} = ''; field('${selector}').focus()`);
    await press('v');
    await poll(`${selector} to receive the pasted text`, async () =>
      (await page('', `field('${selector}').${property}`)) === text);
  }

  assert.equal(await page('', 'window.pastes'), 0, 'Chromium pasted on its own');
  pass(`${backend}: Ctrl+V pasted GPUI's clipboard into a text input, a password input, a textarea and a contenteditable`);

  const copyReport = since => app.wait('the page copy report', event =>
    event.type === 'page_copy' && event.tab === tab, since, stepTimeoutMs);

  const copy = async (key, step) => {
    await clipboard(sentinel);
    const since = app.events.length;
    await page(step);
    await press(key);

    return (await copyReport(since)).copied;
  };

  const select = (selector, value) => `field('${selector}').value = '${value}'; field('${selector}').focus(); field('${selector}').select()`;

  assert.equal(await copy('c', select('#password', 'flecha-copy-secret')), false, 'Ctrl+C read a password field');
  assert.equal(await clipboard(), sentinel, 'Ctrl+C put a password on the clipboard');
  assert.equal(await copy('x', select('#password', 'flecha-cut-secret')), false, 'Ctrl+X read a password field');
  assert.equal(await clipboard(), sentinel, 'Ctrl+X put a password on the clipboard');
  assert.equal(await page('', `field('#password').value`), 'flecha-cut-secret', 'Ctrl+X deleted a password');
  // Nothing was copied, so the page got the held keystrokes, as it would without Flecha.
  await poll('the held Ctrl+C and Ctrl+X to reach the page', async () => (await page('', 'window.editKeys')) === 2);
  pass(`${backend}: Ctrl+C and Ctrl+X in a password field left GPUI's clipboard and the field unchanged and reached the page as keystrokes`);

  assert.equal(await copy('c', select('#text', 'flecha-copy-text')), true, 'Ctrl+C read nothing');
  assert.equal(await clipboard(), 'flecha-copy-text', 'Ctrl+C did not copy the input text');
  assert.equal(await copy('c', `
    document.activeElement.blur();
    getSelection().selectAllChildren(document.querySelector('h1'))`), true, 'Ctrl+C read no page selection');
  assert.equal(await clipboard(), 'Flecha fixture /clipboard', 'Ctrl+C did not copy the page text');
  assert.equal(await copy('x', select('#area', 'flecha-cut-area')), true, 'Ctrl+X read nothing');
  assert.equal(await clipboard(), 'flecha-cut-area', 'Ctrl+X did not copy the textarea text');
  await poll('the cut text to leave the textarea', async () => (await page('', `field('#area').value`)) === '');
  assert.equal(await copy('x', `
    field('#editable').textContent = 'flecha-edit-keepflecha-edit-cut';
    field('#editable').focus();
    getSelection().setBaseAndExtent(field('#editable').firstChild, 16, field('#editable').firstChild, 31)`), true,
  'Ctrl+X read nothing in the contenteditable');
  assert.equal(await clipboard(), 'flecha-edit-cut', 'Ctrl+X did not copy the contenteditable text');
  await poll('the cut text to leave the contenteditable', async () =>
    (await page('', `field('#editable').textContent`)) === 'flecha-edit-keep');
  assert.equal(await page('', 'window.editKeys'), 2, 'a copy or cut with text also reached the page as a keystroke');
  pass(`${backend}: Ctrl+C copied input and page selections and Ctrl+X cut textarea and contenteditable text to GPUI's clipboard without Chromium's own copy`);

  // Selections the read cannot tie to a focused field fail closed: Chromium copies a password's
  // mask from them.
  assert.equal(await copy('c', `
    shadowPassword.value = 'flecha-shadow-secret';
    shadowPassword.focus();
    shadowPassword.select()`), false, 'Ctrl+C read a password in a closed shadow root');
  assert.equal(await clipboard(), sentinel, 'Ctrl+C put a hidden password mask on the clipboard');
  assert.equal(await copy('c', `${select('#password', 'flecha-blur-secret')}; document.activeElement.blur()`), false,
    'Ctrl+C read a blurred password selection');
  assert.equal(await clipboard(), sentinel, 'Ctrl+C put a blurred password mask on the clipboard');
  const checkbox = `${select('#password', 'flecha-check-secret')}; field('#check').focus()`;

  assert.equal(await copy('c', checkbox), false, 'Ctrl+C read a password selection behind a focused checkbox');
  assert.equal(await clipboard(), sentinel, 'Ctrl+C put a password mask behind a focused checkbox on the clipboard');
  assert.equal(await copy('x', checkbox), false, 'Ctrl+X read a password selection behind a focused checkbox');
  assert.equal(await clipboard(), sentinel, 'Ctrl+X put a password mask behind a focused checkbox on the clipboard');
  assert.equal(await page('', `field('#password').value`), 'flecha-check-secret', 'Ctrl+X deleted a password behind a focused checkbox');
  await poll('the held Ctrl+C and Ctrl+X keystrokes to reach the page', async () => (await page('', 'window.editKeys')) === 6);
  assert.equal(await copy('c', select('#email', 'flecha@copy.test')), true, 'Ctrl+C read no email field');
  assert.equal(await clipboard(), 'flecha@copy.test', 'Ctrl+C did not copy the email field');
  assert.equal(await copy('x', `${select('#text', 'flecha-read-only')}; field('#text').readOnly = true`), false,
    'Ctrl+X cut from a read-only field');
  assert.equal(await clipboard(), sentinel, 'Ctrl+X in a read-only field wrote the clipboard');
  assert.equal(await page('field("#text").readOnly = false', `field('#text').value`), 'flecha-read-only', 'Ctrl+X changed a read-only field');
  await poll('the held Ctrl+X to reach the page', async () => (await page('', 'window.editKeys')) === 7);
  pass(`${backend}: Ctrl+C and Ctrl+X refused hidden or blurred password selections, Ctrl+X refused a read-only field, and Ctrl+C copied an email field`);

  // The page menu opens on a right-click that CEF receives, with the field's selection intact.
  // A row label no menu has tells an open menu from none. Any other answer is a failure.
  const openMenu = async (x, y) => {
    await app.command('click', { tab, x, y, right: true });
    await poll('the page menu to open', async () => {
      const { error } = await app.attempt('menu_row', { label: 'Menu probe' });

      if (error === 'no page menu is open') return undefined;
      assert.equal(error, 'the page menu has no enabled Menu probe row');

      return true;
    });
  };

  const menu = async (selector, label) => {
    await openMenu(...await center(selector));

    return app.attempt('menu_row', { label });
  };

  await clipboard('flecha-menu-paste');
  await page(`field('#text').value = ''`);
  assert.equal((await menu('#text', 'Paste')).error, undefined);
  await poll('the page menu paste', async () => (await page('', `field('#text').value`)) === 'flecha-menu-paste');
  assert.equal(await page('', 'window.pastes'), 0, 'Chromium pasted on its own');

  await clipboard(sentinel);
  await page(select('#text', 'flecha-menu-copy'));
  assert.equal((await menu('#text', 'Copy')).error, undefined);
  await poll('the page menu copy', async () => (await clipboard()) === 'flecha-menu-copy');

  await clipboard(sentinel);
  await page(select('#area', 'flecha-menu-cut'));
  assert.equal((await menu('#area', 'Cut')).error, undefined);
  await poll('the page menu cut', async () => (await clipboard()) === 'flecha-menu-cut');
  await poll('the page menu cut to leave the textarea', async () => (await page('', `field('#area').value`)) === '');

  await clipboard(sentinel);
  await page(select('#password', 'flecha-menu-secret'));
  assert.equal((await menu('#password', 'Copy')).error, 'the page menu has no enabled Copy row');
  assert.equal((await app.attempt('menu_row', { label: 'Cut' })).error, 'the page menu has no enabled Cut row');
  await app.command('key', { keystroke: 'escape' });
  assert.equal(await clipboard(), sentinel, 'the page menu put a password on the clipboard');
  pass(`${backend}: the page menu pasted from, copied to and cut to GPUI's clipboard, and refused to copy or cut a password`);

  // A cross-site frame takes pasted text from the keyboard and from a menu opened in it. The
  // selection read cannot see into it, so Copy fails closed and the frame gets the keystroke.
  await clipboard('flecha-frame-paste');
  await frame('focus', state => state.active === 'text');
  await press('v');
  await poll('the frame field to receive the pasted text', async () => (await frameState()).text === 'flecha-frame-paste');

  await clipboard('flecha-frame-menu-paste');
  const { areaCenter } = await frame('clear-area', state => state.active === 'area' && state.area === '');
  await page(`field('#text').value = ''; field('#text').focus()`);

  const [left, top] = await page('', `(() => {
    const box = field('#frame').getBoundingClientRect();
    return [box.x + field('#frame').clientLeft, box.y + field('#frame').clientTop];
  })()`);

  await openMenu(left + areaCenter[0], top + areaCenter[1]);
  assert.equal((await app.attempt('menu_row', { label: 'Paste' })).error, undefined);
  await poll('the frame menu paste', async () => (await frameState()).area === 'flecha-frame-menu-paste');
  assert.equal(await page('', `field('#text').value`), '', 'the frame menu paste reached the parent page');

  const { editKeys } = await frame('select', state =>
    state.active === 'text' && state.selection[0] === 0 && state.selection[1] === 'flecha-frame-copy'.length);

  assert.equal(await copy('c', ''), false, 'Ctrl+C read into a cross-site frame');
  assert.equal(await clipboard(), sentinel, 'Ctrl+C in a cross-site frame wrote the clipboard');
  await poll('the held Ctrl+C to reach the frame', async () => (await frameState()).editKeys === editKeys + 1);
  pass(`${backend}: Ctrl+V and the page menu pasted into a cross-site frame, and Ctrl+C there left the clipboard alone and reached the frame`);

  // Races between a copy or cut and the page. Each check reports its own failure, so a run
  // against an older build shows every check it fails. A cut must copy exactly what it deletes.
  const failed = [];

  const check = async (name, run) => {
    try {
      await run();
      pass(`${backend}: ${name}`);
    } catch (error) {
      failed.push(name);
      console.error(`FAIL ${name} while waiting for ${waitingFor}`, error);
    }
  };

  // Whether `value` is `original` with `clip` taken out in one piece.
  const cutFrom = (original, value, clip) => {
    for (let at = 0; at <= value.length; at++) {
      if (value.slice(0, at) + clip + value.slice(at) === original) return true;
    }

    return false;
  };

  // The verifier lost text in 17 to 19 of 20 such cuts on cc0e2dd, so six trials catch it.
  await check('keys typed right after Ctrl+X move no cut text', async () => {
    const original = 'flecha-race-cut flecha-race-keep';

    for (let trial = 0; trial < 6; trial++) {
      await clipboard(sentinel);
      await page(`field('#text').value = '${original}'; field('#text').focus(); field('#text').setSelectionRange(0, 15)`);
      const since = app.events.length;
      await keys(['Control_L', 'x'], ['End'], ['Shift_L', 'Left'], ['Shift_L', 'Left']);
      const { copied } = await copyReport(since);
      const value = await page('', `field('#text').value`);
      const clip = await clipboard();
      const consistent = copied ? cutFrom(original, value, clip) : value === original && clip === sentinel;

      assert(consistent, `trial ${trial} copied ${JSON.stringify(clip)} and left ${JSON.stringify(value)}`);
    }
  });

  // A page that moves focus between two selections as often as it can. The verifier lost text in
  // 8 and in 34 of 40 such cuts on cc0e2dd, so twenty trials catch it.
  await check('focus moving during Ctrl+X moves no cut text', async () => {
    const [first, second] = ['flecha-race-keepA flecha-race-cut', 'flecha-race-keepB'];

    try {
      for (let trial = 0; trial < 20; trial++) {
        await clipboard(sentinel);
        await page(`
          clearInterval(window.swap);
          field('#area').value = '${first}';
          field('#area2').value = '${second}';
          field('#area').focus();
          field('#area').setSelectionRange(18, 33);
          window.swap = setInterval(() => {
            const [to, start, end] = document.activeElement === field('#area') ? ['#area2', 0, 6] : ['#area', 18, 33];
            field(to).focus();
            field(to).setSelectionRange(start, end);
          }, 0)`);
        const since = app.events.length;
        await press('x');
        const { copied } = await copyReport(since);
        const [a, b] = await page('clearInterval(window.swap)', `[field('#area').value, field('#area2').value]`);
        const clip = await clipboard();

        const consistent = copied
          ? (cutFrom(first, a, clip) && b === second) || (a === first && cutFrom(second, b, clip))
          : a === first && b === second && clip === sentinel;

        assert(consistent, `trial ${trial} copied ${JSON.stringify(clip)} and left ${JSON.stringify([a, b])}`);
      }
    } finally {
      await page('clearInterval(window.swap)');
    }
  });

  // A handler that moves the selection while the cut deletes. The cut copies what left the field.
  await check('a page handler moving the selection during Ctrl+X moves no cut text', async () => {
    const original = 'flecha-handler-keep flecha-handler-cut';

    await clipboard(sentinel);
    await page(`
      field('#area').value = '${original}';
      field('#area').focus();
      field('#area').setSelectionRange(20, 38);
      field('#area').addEventListener('beforeinput', () => field('#area').setSelectionRange(0, 6), { once: true })`);
    const since = app.events.length;
    await press('x');
    assert.equal((await copyReport(since)).copied, true, 'Ctrl+X read nothing');
    const value = await page('', `field('#area').value`);
    const clip = await clipboard();

    assert(cutFrom(original, value, clip), `copied ${JSON.stringify(clip)} and left ${JSON.stringify(value)}`);
  });

  // The top page forges the read while the user's selection is a password in the cross-site
  // frame. cc0e2dd deleted that password.
  await check('a forged selection over a cross-site password deletes nothing there', async () => {
    const secret = 'flecha-frame-secret';

    await frame('select-password', state =>
      state.active === 'password' && state.selection[0] === 0 && state.selection[1] === secret.length);
    await clipboard(sentinel);

    try {
      const since = app.events.length;
      await page(`
        window.realActive = Object.getOwnPropertyDescriptor(Document.prototype, 'activeElement');
        window.realSelection = Document.prototype.getSelection;
        Object.defineProperty(Document.prototype, 'activeElement', { get: () => null, configurable: true });
        Document.prototype.getSelection = () => ({ isCollapsed: false, toString: () => 'flecha-forged' })`);
      await press('x');
      await copyReport(since);
    } finally {
      await page(`
        Object.defineProperty(Document.prototype, 'activeElement', window.realActive);
        Document.prototype.getSelection = window.realSelection`);
    }

    assert.equal((await frameState()).password, secret, 'Ctrl+X over a forged selection deleted the frame password');
    assert.notEqual(await clipboard(), secret, 'Ctrl+X put the frame password on the clipboard');
  });

  // The page swaps the selection between the right-click and Cut. cc0e2dd copied the old
  // selection and deleted the new one.
  await check('the page menu cuts the selection it copies', async () => {
    await clipboard(sentinel);
    await page(select('#area', 'flecha-race-cut'));
    await openMenu(...await center('#area'));
    await page(`field('#area').value = 'flecha-race-keep\\n' + field('#area').value; field('#area').setSelectionRange(0, 16)`);
    assert.equal((await app.attempt('menu_row', { label: 'Cut' })).error, undefined);

    const clip = await poll('the page menu cut', async () => {
      const text = await clipboard();

      return text !== sentinel && text;
    });

    assert.equal(clip, 'flecha-race-keep', 'the page menu did not cut the current selection');
    assert.equal(await page('', `field('#area').value`), '\nflecha-race-cut', 'the page menu deleted other text');
  });

  // The page moves focus into the cross-site frame between the right-click and Paste. cc0e2dd
  // pasted into the frame. A keyboard paste afterwards shows whether the menu's text landed first.
  await check('the page menu pastes only into its own frame', async () => {
    await clipboard('flecha-moved-paste');
    await page(`field('#area').value = ''; field('#area').focus()`);
    await openMenu(...await center('#area'));
    await frame('clear-area', state => state.active === 'area' && state.area === '');
    assert.equal((await app.attempt('menu_row', { label: 'Paste' })).error, undefined);
    await clipboard('flecha-later-paste');
    await press('v');

    const { area } = await poll('the keyboard paste in the frame', async () => {
      const state = await frameState();

      return state.area.includes('flecha-later-paste') && state;
    });

    assert.equal(area, 'flecha-later-paste', 'the page menu pasted into the frame that took focus');
    assert.equal(await page('', `field('#area').value`), '', 'the page menu pasted after its frame lost focus');
  });

  // Keys and pastes after Ctrl+X while the page runs a long task. They must act after the cut,
  // as in Chromium. The verifier lost the cut text in 10 of 10 trials at every busy length from
  // 30 to 300 ms on 5203212.
  const busyCut = (name, after, expected) => check(`${name} on a busy page acts after the cut`, async () => {
    const original = 'flecha-race-cut flecha-race-keep';

    for (const busy of [30, 100, 300]) {
      for (let trial = 0; trial < 2; trial++) {
        await clipboard('flecha-old-clip');
        await page(`
          field('#text').value = '${original}';
          field('#text').focus();
          field('#text').setSelectionRange(0, 15);
          setTimeout(() => { const start = performance.now(); while (performance.now() - start < ${busy}); }, 20)`);
        await delay(40); // Inside the long task, which starts 20 ms after the step.
        const since = app.events.length;

        await keys(['Control_L', 'x'], ...after);
        await copyReport(since);
        let value;

        try {
          await poll(`the field after ${name}`, async () => (value = await page('', `field('#text').value`)) === expected);
        } catch {
          assert.fail(`busy ${busy} ms, trial ${trial}: the field reads ${JSON.stringify(value)}, not ${JSON.stringify(expected)}`);
        }

        assert.equal(await clipboard(), 'flecha-race-cut', `busy ${busy} ms, trial ${trial}: the cut text is not on the clipboard`);
      }
    }
  });

  await busyCut('typing after Ctrl+X', [['a'], ['b']], 'ab flecha-race-keep');
  await busyCut('Ctrl+V after Ctrl+X', [['Control_L', 'v']], 'flecha-race-cut flecha-race-keep');
  await busyCut('End and Ctrl+V after Ctrl+X', [['End'], ['Control_L', 'v']], ' flecha-race-keepflecha-race-cut');

  assert.deepEqual(failed, [], 'clipboard race checks failed');

  if (x11) {
    execFileSync('setxkbmap', ['-layout', 'us', '-variant', 'intl', '-option', 'compose:menu']);
    const text = `field('#text').value`;
    const focus = `field('#text').value = ''; field('#text').focus()`;
    await page(`${focus};
      window.compositionEvents = [];
      for (const type of ['compositionstart', 'compositionend']) {
        field('#text').addEventListener(type, event => window.compositionEvents.push([type, event.data]));
      }`);
    await keys(['a'], ['dead_acute']);
    await poll('the dead key to start page composition', async () => (await page('', text)) === 'a´');
    await keys(['e'], ['Multi_key'], ['o'], ['c']);
    await poll('the composed characters to commit once', async () => (await page('', text)) === 'aé©');
    assert.deepEqual(await page('', 'window.compositionEvents'), [['compositionstart', ''], ['compositionend', 'é']]);
    pass('X11: ordinary typing, a dead-key accent and a Compose sequence each inserted once');

    const next = await openTab(app, '/clipboard');
    await page(focus, 'null', next);
    await app.command('select', { tab });
    await app.present();
    await page(focus);
    await keys(['dead_acute']);
    await poll('the second dead key to start composition', async () => (await page('', text)) === '´');
    await app.command('select', { tab: next });
    await app.present();
    await poll('the original tab to discard preedit', async () => (await page('', text)) === '');
    await keys(['e']);
    const typed = await poll('typing in the next tab to reach the field', async () => page('', text, next));

    assert.equal(typed, 'e', 'the previous tab sent its Compose sequence to the next tab');
    pass('X11: a tab switch discarded the preedit and reset the Compose sequence');

    await keys(['dead_acute']);
    await poll('a dead key to mark text before a pointer press', async () => (await page('', text, next)) === 'e´');
    await clickElement(app, next, '#text', ['#147A70', '#FFCC49'], 'ime-field', opened.window);
    await poll('the pointer press to discard preedit', async () => (await page('', text, next)) === 'e');
    pass('X11: a pointer press discarded preedit without inserting the accent');
  }

  await app.quit();
}

try {
  if (mode === 'adblock') {
    await verifyAdblock();
  } else if (mode === 'clipboard') {
    await verifyClipboard();
  } else {
    await verifySlice();
  }
} catch (error) {
  console.error(`FAIL while waiting for ${waitingFor}`, error);
  process.exitCode = 1;
} finally {
  for (const app of apps) app.release();
  site.server.closeAllConnections();
  site.server.close();
}
