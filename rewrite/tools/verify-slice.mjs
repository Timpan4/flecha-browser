import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { once, EventEmitter } from 'node:events';
import { createWriteStream, readFileSync, renameSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';

import { blankRestartColor, fixture } from './fixture.mjs';
import { checkProcessEnvironment } from './verify-process-env.mjs';

const out = join(process.env.HOME, '..');
const x11Driver = join(out, 'x11-input');
const mode = process.argv[2] ?? 'slice';

const shippingTrace = readFileSync(new URL('./slice-ts-replay.jsonl', import.meta.url), 'utf8')
  .trim().split('\n').map(line => JSON.parse(line));

assert(['slice', 'open', 'focus', 'break-restore'].includes(mode), 'unknown slice check');

const site = await fixture();
const { base } = site;
const pass = message => console.log(`PASS ${message}`);
let waitingFor = 'start Flecha';

function browser(name) {
  const child = spawn('./target/debug/flecha', ['--verify']);
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

  async function wait(description, match, since = 0) {
    waitingFor = description;

    for (;;) {
      const event = events.slice(since).find(match);

      if (event) return event;
      assert(!exit, `Flecha exited before ${description}: ${JSON.stringify(exit)}`);
      await once(emitter, 'message');
    }
  }

  async function command(op, detail = {}) {
    const request = ++seq;
    child.stdin.write(JSON.stringify({ seq: request, op, ...detail }) + '\n');
    const reply = await wait(`reply to ${op}`, event => event.type === 'reply' && event.seq === request);
    assert.equal(reply.error, undefined, reply.error);

    return reply.result;
  }

  return {
    events, command, wait, pid: child.pid,
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
    async present() {
      const after = events.length;
      await command('capture');
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
  const sidebar = 0x202a30;
  // The sidebar's first row starts below the command bar. Its padding
  // leaves this entire row solid, so both chrome edges come from the
  // screenshot independently of the page layout.
  const [windowX, windowY, windowWidth, windowHeight] = geometry;
  const origin = Math.max(0, windowX);
  const end = Math.min(width, windowX + windowWidth);
  const bottom = Math.min(height, windowY + windowHeight);
  let top = Math.max(0, windowY);

  while (top < bottom && pixel(origin, top) !== sidebar) top++;

  if (top >= bottom) return 'could not locate the sidebar';

  let edge = origin;

  while (edge < end && pixel(edge, top) === sidebar) edge++;

  if (edge >= end) return 'could not locate the page area';

  // The split has no gutters or borders. A fractional boundary can
  // rasterize either way in the single column it crosses.
  const boundary = edge + (windowX + windowWidth - edge) * ratio;
  const leftEnd = Math.floor(boundary);
  const rightStart = Math.ceil(boundary);

  for (let y = 0; y < height; y++) {
    for (let x = 0; x < width; x++) {
      const actual = pixel(x, y);
      let matches = actual !== left && actual !== right;

      if (y >= top && y < bottom && x >= edge && x < end) {
        matches = x < leftEnd ? actual === left : x >= rightStart ? actual === right : actual === left || actual === right;
      }

      if (!matches) {
        return `unexpected #${actual.toString(16)} at (${x}, ${y}) in ${width}×${height}, page starts at (${edge}, ${top}) and splits at ${boundary}`;
      }
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

async function captureUntil(name, window, mismatch) {
  waitingFor = `${name} capture`;
  const path = join(out, `${name}.png`);
  let previous;

  // X can still expose the previous frame after GPUI acknowledges presentation.
  // The launcher watchdog bounds both the capture attempts and their subprocesses.
  for (;;) {
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

    await delay(100); // Matches Spike 1's capture readiness polling.
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
    '-lX11', '-lXtst', '-o', x11Driver,
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

  waitingFor = `X11 key after sidebar ${step}`;
  execFileSync(x11Driver, ['key', key]);
  const actual = await received;

  assert.equal(actual, expected, `sidebar ${step} sent ${key} to pane ${actual}, expected ${expected}`);

  for (const [name, tab] of Object.entries(tabs)) {
    const report = site.probe(`${step}-${name}-report`);

    await app.command('script', {
      tab,
      source: `fetch('/probe/${step}-${name}-report?value=' + JSON.stringify(window.probeKeys));`,
    });
    assert.deepEqual(JSON.parse(await report), name === expected ? [key] : [], `keys received by pane ${name}`);
  }

  pass(`sidebar ${step} sent ${key} only to pane ${expected}`);
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

  // Xvfb fixture geometry with one Space: the page marker, the first tab's
  // Split button, and the second tab's label.
  execFileSync(x11Driver, ['click', 340, 110]);
  execFileSync(x11Driver, ['click', 235, 189]);
  const split = await waitForSelection(app, right, 'sidebar Split selected pane B');

  assert.deepEqual(active(split).split, [left, right]);
  await keyDelivery(app, { A: left, B: right }, 'y', 'split', 'B');

  execFileSync(x11Driver, ['click', 95, 227]);
  await waitForSelection(app, left, 'sidebar selected pane A');
  await keyDelivery(app, { A: left, B: right }, 'z', 'select', 'A');
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

  // The verifier measured this point inside the Separate cookies row with two Spaces.
  execFileSync(x11Driver, ['click', 95, 189]);

  for (;;) {
    const state = await app.command('snapshot');
    assert.equal(isIsolated(state), false, 'Peek backdrop passed the click through to Separate cookies');

    if (!state.peeks.length) break;
    await delay(100); // Matches the capture poll.
  }

  await app.wait('CEF closure after clicking the Peek backdrop', event =>
    event.type === 'closed' && event.tab === clickedPeek);
  pass('clicking Separate cookies through the Peek backdrop dismissed Peek and left the Space shared');

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

const apps = [];

function launch(name) {
  const app = browser(name);
  apps.push(app);

  return app;
}

try {
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
  await nativeInput(app, left);

  if (mode === 'focus') {
    await sidebarFocus(app, left);
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
    const closed = await app.command('close');
    assert(!closed.core.tabs[moved]);
    assert.deepEqual(closed.core.archive.slice(1), returned.core.archive, 'closing did not add a fresh archive entry');
    const entry = closed.core.archive[0];
    assert.deepEqual([entry.url, entry.title], [closeUrl, closeTitle], 'new archive entry belongs to a different page');
    assert(returned.core.archive.every(old => entry.closedAt > old.closedAt), 'new archive entry does not have a later close time');
    assert(entry.closedAt >= closeStartedAt, 'archive timestamp predates the close command');
    compareShipping('close', closed, names);
    await app.wait('CEF closure of returned tab', event => event.type === 'closed' && event.tab === moved);
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
    await restarted.quit();
  } else {
    await app.quit();
  }

  pass('CEF shut down cleanly');
} catch (error) {
  console.error(`FAIL while waiting for ${waitingFor}`, error);
  process.exitCode = 1;
} finally {
  for (const app of apps) app.release();
  site.server.closeAllConnections();
  site.server.close();
}
