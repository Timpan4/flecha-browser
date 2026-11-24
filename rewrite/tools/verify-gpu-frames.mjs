// Checks page frames on a GPU under headless weston: shared textures for a page, a
// <select> popup, split view, Peek, a window resize and a scripted popup window, or, in
// `fallback` mode, the switch to CPU copies after an import fails. Run it through
// verify-gpu-frames.sh.
import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { EventEmitter, once } from 'node:events';
import { createWriteStream, readdirSync, readFileSync, renameSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';

import { fixture, popupColors, selectColors, selectOptionColor } from './fixture.mjs';

const out = join(process.env.HOME, '..');
const mode = process.argv[2] ?? 'frames';

assert(['frames', 'fallback'].includes(mode), 'unknown GPU frame check');

// The fixture's #marker is 116 x 56 CSS pixels, and weston's output scale is 1. A
// frame drawn at another size than the pane, or stretched, changes this count.
const markerPixels = 116 * 56;
const markers = { open: '#FFCC49', split: '#F386B9', peek: '#83DDFF', select: selectColors[1], popup: popupColors[1] };
const backgrounds = ['#147A70', '#703496'];

const site = await fixture();
const { base } = site;
const pass = message => console.log(`PASS ${message}`);
const logPath = join(out, 'gpu-frames.log');

function launch() {
  const child = spawn('./target/debug/flecha', ['--verify']);
  const log = createWriteStream(logPath);
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

  async function wait(description, match, since = 0) {
    for (;;) {
      const event = events.slice(since).find(match);

      if (event) return event;
      assert(!exit, `Flecha exited before ${description}: ${JSON.stringify(exit)}`);
      await once(emitter, 'message');
    }
  }

  async function command(op, detail = {}) {
    const request = ++seq;
    const sent = events.length;
    child.stdin.write(JSON.stringify({ seq: request, op, ...detail }) + '\n');

    const reply = await wait(`reply to ${op}`, event =>
      (event.type === 'reply' && event.seq === request) || event.type === 'invalid_request', sent);

    assert.notEqual(reply.type, 'invalid_request', `Flecha does not understand the ${op} request`);
    assert.equal(reply.error, undefined, reply.error);

    return reply.result;
  }

  return {
    events, command, wait,
    loaded: (tab, path, since = 0) => wait(`HTTP load of ${path}`, event =>
      event.type === 'loaded' && event.tab === tab && event.url === base + path, since),
    async present() {
      const after = events.length;
      await command('capture');
      await wait('GPUI presented a frame', event => event.type === 'presented', after);
    },
    async quit() {
      await command('quit');
      child.stdin.end();
      assert.deepEqual(await closed, [0, null]);
    },
  };
}

function histogram(path) {
  const counts = new Map();

  // ImageMagick warns that /etc has no colors.xml inside the launcher.
  const text = execFileSync('magick', [path, '-format', '%c', 'histogram:info:'],
    { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] });

  for (const line of text.split('\n')) {
    const match = /^\s*(\d+):.*(#[0-9A-F]{6})\b/i.exec(line);

    if (match) counts.set(match[2].toUpperCase(), Number(match[1]));
  }

  return counts;
}

let step = 0;

// Retries until `mismatch` accepts a capture. The launcher watchdog bounds the attempts.
async function capture(app, name, mismatch) {
  step += 1;
  let previous;

  for (;;) {
    await app.present();
    execFileSync('weston-screenshooter', { cwd: out, stdio: 'ignore' });
    const shot = readdirSync(out).find(file => file.startsWith('wayland-screenshot-'));
    const path = join(out, `${String(step).padStart(2, '0')}-${name}.png`);
    renameSync(join(out, shot), path);
    const counts = histogram(path);
    const reason = mismatch(counts);

    if (!reason) return counts;

    if (reason !== previous) console.error(`RETRY ${name}: ${reason}`);
    previous = reason;
    await delay(100); // Matches verify-slice.mjs capture readiness polling.
  }
}

const frameLines = () => readFileSync(logPath, 'utf8').split('\n').filter(line => line.startsWith('page frames: '));

// Polls the log until a line matches. The launcher watchdog bounds the wait.
async function logged(pattern) {
  for (;;) {
    const line = frameLines().find(line => pattern.test(line));

    if (line) return line;
    await delay(100); // Matches verify-slice.mjs capture readiness polling.
  }
}

const exactly = (...colors) => counts => {
  const wrong = colors.filter(color => counts.get(color) !== markerPixels);

  return wrong.length
    ? `marker pixels ${wrong.map(color => `${color}=${counts.get(color) ?? 0}`).join(', ')}, want ${markerPixels} each`
    : undefined;
};

const active = snapshot => {
  const window = snapshot.core.windows.find(window => window.id === snapshot.window);

  return window.spaces[window.activeSpaceId];
};

const app = launch();
const browserOf = (tab, since = 0) => app.wait(`the browser for ${tab}`,
  event => event.type === 'created' && event.tab === tab, since);

await app.wait('CEF context ready', event => event.type === 'ready');

// Weston's headless output is 1024 x 640 with a 32 px panel. Fit the window inside it.
await app.command('resize', { width: 1000, height: 600 });

const opened = await app.command('open', { url: base + '/select' });
const selectTab = active(opened).activeTabId;

await app.loaded(selectTab, '/select');

await capture(app, 'select-page', exactly(markers.select));

pass('the /select page rendered at its own size');

if (mode === 'fallback') {
  const { browser } = await browserOf(selectTab);

  await logged(new RegExp(`^page frames: browser ${browser} receives shared textures$`));

  // Fails the import of the next frame, which the heading change paints.
  await app.command('reject_next_shared_texture');

  const since = app.events.length;

  await app.command('script', { tab: selectTab, source: 'document.querySelector("h1").textContent = "Repainted"' });

  await logged(/^page frames: reopening a page with CPU copies: /);

  await logged(/^page frames: CPU copies for new browsers from now on: /);

  const reopened = await browserOf(selectTab, since);

  assert.notEqual(reopened.browser, browser, 'the page was not reopened');

  await app.loaded(selectTab, '/select', since);

  await logged(new RegExp(`^page frames: browser ${reopened.browser} receives CPU copies$`));

  await capture(app, 'fallback-select', exactly(markers.select));

  pass('after a failed import the page reopened with CPU copies at its own size');

  const split = await app.command('split', { url: base + '/split' });
  const [, right] = active(split).split;

  await app.loaded(right, '/split');

  await capture(app, 'fallback-split', exactly(markers.select, markers.split));

  const lines = frameLines();
  const afterFailure = lines.slice(lines.findIndex(line => line.startsWith('page frames: reopening')));

  assert(!afterFailure.some(line => /receives shared textures/.test(line)),
    `a browser received shared textures after the failure:\n${lines.join('\n')}`);

  pass(`the next browser used CPU copies too:\n  ${lines.join('\n  ')}`);

  await app.quit();

  pass('Flecha quit cleanly after the fallback');

  process.exit(0);
}

// The <select> sits at (400, 32) with a 240 x 48 box in page pixels.
await app.command('click', { tab: selectTab, x: 520, y: 56 });

await capture(app, 'select-open', counts =>
  (counts.get(selectOptionColor) ?? 0) > 0 ? undefined : 'no option rows in the capture');

pass('a <select> popup rendered over the page');

await app.command('click', { tab: selectTab, x: 520, y: 56 });

await capture(app, 'select-closed', counts =>
  counts.has(selectOptionColor) ? 'option rows are still visible' : undefined);

pass('the <select> popup closed');

const page = await app.command('open', { url: base + '/open' });
const left = active(page).activeTabId;

await app.loaded(left, '/open');

await capture(app, 'open', exactly(markers.open));

pass('a page rendered at its own size');

const split = await app.command('split', { url: base + '/split' });
const [, right] = active(split).split;

await app.loaded(right, '/split');

const before = await capture(app, 'split', exactly(markers.open, markers.split));

pass('split view rendered both pages at their pane sizes');

const peek = await app.command('peek', { url: base + '/peek' });
const [{ tab: peekTab }] = peek.peeks;

await app.loaded(peekTab, '/peek');

await capture(app, 'peek', exactly(markers.peek));

await app.command('close_peek');

pass('Peek rendered its page');

const area = counts => backgrounds.reduce((sum, color) => sum + (counts.get(color) ?? 0), 0);

await app.command('resize', { width: 800, height: 520 });

const after = await capture(app, 'resized', counts => exactly(markers.open, markers.split)(counts)
  ?? (area(counts) < area(before) ? undefined : `page area ${area(counts)} did not shrink from ${area(before)}`));

pass(`after a resize both pages rendered at their new pane sizes (page area ${area(before)} -> ${area(after)} pixels)`);

// A scripted window.open creates its browser through OnBeforePopup, in a second GPUI window.
const openerPage = await app.command('open', { url: base + '/opener' });
const opener = active(openerPage).activeTabId;

await app.loaded(opener, '/opener');

await app.command('script', {
  tab: opener,
  source: 'document.querySelector("#scripted").style.cssText = "position: absolute; left: 400px; top: 32px; width: 240px; height: 48px"',
});

const clicked = app.events.length;

await app.command('click', { tab: opener, x: 520, y: 56 });

const popupLoad = await app.wait('the popup load', event => event.type === 'loaded' && event.url === base + '/popup', clicked);

await capture(app, 'popup-window', exactly(markers.popup));

const popup = await browserOf(popupLoad.tab);

await logged(new RegExp(`^page frames: browser ${popup.browser} receives shared textures$`));

pass('a scripted popup window rendered its page from shared textures');

const lines = frameLines();

assert.match(lines[0] ?? '', /^page frames: shared textures on \/dev\/dri\/renderD\d+$/, lines.join('\n'));

assert(lines.some(line => /receives shared textures/.test(line)), lines.join('\n'));

assert(!lines.some(line => /CPU copies/.test(line)), `a browser fell back:\n${lines.join('\n')}`);

pass(`every browser received shared textures:\n  ${lines.join('\n  ')}`);

await app.quit();

pass('Flecha quit cleanly');

site.server.close();
