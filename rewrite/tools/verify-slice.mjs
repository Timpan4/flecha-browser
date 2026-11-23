import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { once, EventEmitter } from 'node:events';
import { createWriteStream, renameSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';

import { fixture } from './fixture.mjs';
import { checkProcessEnvironment } from './verify-process-env.mjs';

const out = join(process.env.HOME, '..');
const mode = process.argv[2] ?? 'slice';

assert(['slice', 'open', 'break-restore'].includes(mode), 'unknown slice check');

const site = await fixture();
const { base } = site;
const pass = message => console.log(`PASS ${message}`);
let waitingFor = 'start Flecha';
const binary = process.argv[3] ?? './target/debug/flecha';

function browser(name) {
  const child = spawn(binary, ['--verify']);
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
    release() {
      child.stdin.destroy();
      child.stdout.destroy();
      child.stderr.destroy();
      child.unref();
    },
  };
}

async function capture(name, colors, window = 'root') {
  waitingFor = `${name} frame with colors ${colors.join(', ')}`;
  const path = join(out, `${name}.png`);

  for (;;) {
    execFileSync('magick', ['import', '-window', window, path]);
    const histogram = execFileSync('magick', [path, '-format', '%c', 'histogram:info:'], { encoding: 'utf8' });

    if (colors.every(color => histogram.includes(color))) return;
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
  const driver = join(out, 'x11-input');
  execFileSync('cc', [
    '-std=c17', '-Wall', '-Wextra', '-Werror', 'spikes/gpui-cef-compositor/x11-input.c',
    '-lX11', '-lXtst', '-o', driver,
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
  execFileSync(driver, ['click', Math.round(marker[0] + x), Math.round(marker[1] + y)]);
  execFileSync(driver, ['key', 'x']);
  waitingFor = 'X11 key delivery to the CEF page';
  assert.equal(await typed, 'x');
  pass('an X11 click and key passed through GPUI into the CEF input field');
}

async function cookie(app, expected) {
  const after = app.events.length;
  const snapshot = await app.command('open', { url: base + '/cookie/check' });
  const tab = active(snapshot).activeTabId;
  await app.loaded(tab, '/cookie/check', after);

  const result = await app.wait('cookie fixture response', event =>
    event.type === 'title' && event.tab === tab && ['Cookie present', 'Cookie absent'].includes(event.title), after);

  assert.equal(result.title, expected, 'cookie request used the wrong Space context');

  return tab;
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
  assert.deepEqual(active(opened).today, [left]);
  await app.loaded(left, '/open');
  await app.wait('CEF paint for initial page', event => event.type === 'paint' && event.tab === left);
  const checked = checkProcessEnvironment(app.pid);
  pass(`checked /proc environ for the app and its CEF descendants (${checked} processes, including a renderer or zygote child)`);
  await capture('open', ['#147A70', '#FFCC49']);
  pass('opened a durable tab, received CEF load and paint, and captured the GPUI fixture frame');
  await nativeInput(app, left);

  if (mode !== 'open') {
    const split = await app.command('split', { url: base + '/split' });
    const [first, right] = active(split).split;
    assert.equal(first, left);
    assert.equal(active(split).activeTabId, left);
    await app.loaded(right, '/split');
    await capture('split', ['#147A70', '#FFCC49', '#703496', '#F386B9']);
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
    assert.equal(active(promoted).activeTabId, peekTab);
    assert.equal(engine(promoted, peekTab, 'tab', main), peekBrowser);
    assert.deepEqual(await document(app, peekTab, 'promoted'), peekDocument);
    pass('promoted the transient Peek into Today with its browser, document and draft intact');

    const detached = await app.command('detach');
    const [{ window: mini, tab: moved }] = detached.flechinhas;
    assert(!detached.core.tabs[peekTab]);
    assert(!detached.core.windows.some(window => window.id === mini));
    assert.equal(engine(detached, moved, 'flechinha', mini), peekBrowser);
    assert.deepEqual(await document(app, moved, 'detached'), peekDocument);
    await capture('flechinha', ['#235089', '#83DDFF'], 'Flechinha');
    assert((await app.command('snapshot')).nativeWindows.includes(mini), 'GPUI opened the Flechinha window');
    const returned = await app.command('return', { window: mini });
    assert.deepEqual(returned.flechinhas, []);
    assert.equal(active(returned).activeTabId, moved);
    assert.equal(engine(returned, moved, 'tab', main), peekBrowser);
    assert.deepEqual(await document(app, moved, 'returned'), peekDocument);
    assert(!(await app.command('snapshot')).nativeWindows.includes(mini), 'GPUI closed the emptied Flechinha window');
    pass('moved the live document to Flechinha and back to the last active Flecha window');

    const closed = await app.command('close');
    assert(!closed.core.tabs[moved]);
    assert(closed.core.archive.some(tab => tab.url === base + '/peek'));
    assert.deepEqual(active(closed).split, [left, right]);
    await app.wait('CEF closure of returned tab', event => event.type === 'closed' && event.tab === moved);
    pass('closed the returned tab, archived its URL, and received CEF closure');

    const after = app.events.length;
    const selected = await app.command('select', { tab: left });
    assert.equal(active(selected).activeTabId, left);
    const newest = await app.command('recreate', { tab: left });
    await app.loaded(left, '/open', after);
    const delivered = app.events.slice(after).filter(event => event.type === 'created' && event.tab === left);
    assert.deepEqual(delivered.map(event => event.browser), [newest], 'stale CEF generation reached the shell');
    pass('replaced a live browser twice before dispatch and received only the current generation');

    const isolated = win(await app.command('new_space')).activeSpaceId;
    const isolatedState = await app.command('isolate');
    assert(isolatedState.core.spaces.find(space => space.id === isolated).isolated);
    const written = await app.command('open', { url: base + '/cookie/set' });
    await app.loaded(active(written).activeTabId, '/cookie/set');
    await cookie(app, 'Cookie present');
    await app.command('switch', { space: shared });
    await cookie(app, 'Cookie absent');
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
    pass('restarted on the same scratch profile and restored core state and both CEF split panes');

    await restarted.command('switch', { space: isolated });
    await cookie(restarted, 'Cookie present');
    await restarted.command('switch', { space: shared });
    await cookie(restarted, 'Cookie absent');
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
