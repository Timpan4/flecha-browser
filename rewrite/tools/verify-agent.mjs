// The MCP agent bridge on a desktop launch, inside the isolated launcher's namespaces.
//
// Run it through tools/verify-agent.sh. It starts Flecha without FLECHA_LAUNCHER=isolated, so
// the app serves agents as a desktop launch does. Its loopback port lives in the launcher's
// private network namespace, and its profile in the scratch directory.

import assert from 'node:assert/strict';
import { execFileSync, spawn } from 'node:child_process';
import { once } from 'node:events';
import { createWriteStream, mkdirSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';

import { fixture } from './fixture.mjs';

const out = join(process.env.HOME, '..');
const profile = join(out, 'desktop-profile');
const x11Driver = join(out, 'x11-input');
const site = await fixture();
const pass = message => console.log(`PASS ${message}`);

const env = { ...process.env };

delete env.FLECHA_LAUNCHER;

delete env.FLECHA_PROFILE;

// Agents stay off until Settings lets them in, as in the shell, so the profile starts with
// the switch on. The state file holds its JSON as a string.
mkdirSync(join(profile, 'session'), { recursive: true });

writeFileSync(
  join(profile, 'session', 'state.json'),
  JSON.stringify(JSON.stringify({ version: 3, tabs: {}, spaces: [{ id: 'agents' }], agent: { enabled: true } })),
);

const child = spawn('./target/debug/flecha', ['--profile', profile, '--open', `${site.base}/open`], { env });
const log = createWriteStream(join(out, 'desktop.log'));
const closed = once(child, 'close');

child.stdout.pipe(log, { end: false });

let announced;

const bridge = new Promise((resolve, reject) => {
  announced = reject;
  createInterface({ input: child.stderr }).on('line', line => {
    log.write(line + '\n');
    const found = /^agent bridge: (http:\/\/127\.0\.0\.1:\d+\/mcp) \(bearer token in (.+)\)$/.exec(line);

    if (found) resolve({ endpoint: found[1], tokenPath: found[2] });
  });
});

child.once('close', code => announced(new Error(`Flecha exited (${code}) before its agent bridge started`)));

const { endpoint, tokenPath } = await bridge;

assert(tokenPath.startsWith(profile), `the token is outside the profile: ${tokenPath}`);

const token = readFileSync(tokenPath, 'utf8');

pass(`a desktop launch started the agent bridge at ${endpoint}`);

let id = 0;

async function rpc(method, params, authorization = `Bearer ${token}`) {
  const response = await fetch(endpoint, {
    method: 'POST',
    headers: {
      Authorization: authorization,
      'Content-Type': 'application/json',
      Accept: 'application/json, text/event-stream',
    },
    body: JSON.stringify({ jsonrpc: '2.0', id: ++id, method, params }),
  });

  const body = await response.text();

  if (!response.ok) return { status: response.status, body };
  const data = body.split('\n').find(line => line.startsWith('data:') && line.length > 'data:'.length);

  return { status: response.status, message: JSON.parse(data.slice('data:'.length)) };
}

async function tool(name, args = {}) {
  const { message } = await rpc('tools/call', { name, arguments: args });
  assert(message?.result, `${name} failed: ${JSON.stringify(message)}`);

  return message.result;
}

const text = result => result.content[0].text;

assert.equal((await rpc('tools/list', {}, 'Bearer wrong')).status, 401);

pass('the bridge refused a wrong bearer token');

// The --open page loads before the tools need it.
let tabs;

for (;;) {
  tabs = JSON.parse(text(await tool('list_tabs')));

  if (tabs.some(tab => tab.url === `${site.base}/open` && tab.active)) break;
  await delay(100);
}

pass(`list_tabs showed the launch's page among ${tabs.length} tab(s)`);

const opened = JSON.parse(text(await tool('open_tab', { url: `${site.base}/peek` })));
const tabId = opened.tabId;
const listed = JSON.parse(text(await tool('list_tabs')));
const row = listed.find(tab => tab.id === tabId);

assert.equal(row?.agent, true, 'open_tab left the tab out of the agent folder');

assert.equal(row.active, false, 'open_tab showed the agent tab');

pass(`open_tab opened ${opened.url} in the agent folder without showing it`);

const titled = await tool('wait_for', { tabId, selector: '#marker', timeoutMs: 20000 });

assert.equal(text(titled), 'ready', JSON.stringify(titled));

assert.equal(text(await tool('evaluate', { tabId, expression: 'document.title' })), 'Flecha fixture /peek');

const page = text(await tool('read_page', { tabId }));

assert(page.startsWith('# Flecha fixture /peek\n'), page.slice(0, 200));

pass('wait_for, evaluate and read_page ran on the hidden agent tab through CDP');

await tool('evaluate', { tabId, expression: 'console.warn("agent-console-check")' });

const logs = text(await tool('console_logs', { tabId, level: 'warning' }));

assert(logs.includes('[warning] agent-console-check'), logs);

pass('console_logs reported the page\'s console warning');

await tool('type', { tabId, selector: 'input', text: 'typed by agent' });

assert.equal(text(await tool('evaluate', { tabId, expression: 'document.querySelector("input").value' })), 'typed by agent');

pass('type filled the page\'s input');

const shot = await tool('screenshot', { tabId });
const image = shot.content[0];

assert.equal(image.type, 'image', JSON.stringify(shot).slice(0, 300));

const png = Buffer.from(image.data, 'base64');

assert.equal(png.subarray(1, 4).toString(), 'PNG', 'the screenshot is not a PNG');

const shotPath = join(out, 'agent-screenshot.png');

writeFileSync(shotPath, png);

const colors = execFileSync('magick', [shotPath, '-format', '%c', 'histogram:info:'], { encoding: 'utf8' });

// The /peek fixture's background and marker (tools/fixture.mjs).
assert(['#235089', '#83DDFF'].every(color => colors.includes(color)), 'the screenshot does not show the page');

pass(`screenshot returned a ${png.length}-byte PNG showing the hidden agent tab's page`);

const clicked = JSON.parse(text(await tool('click', { tabId, selector: 'input' })));

assert.deepEqual(clicked, { clicked: 'input', url: `${site.base}/peek` });

pass('click pressed the page\'s input with a real mouse event');

assert.equal(text(await tool('close_tab', { tabId })), 'ok');

assert(!JSON.parse(text(await tool('list_tabs'))).some(tab => tab.id === tabId));

pass('close_tab closed the agent tab');

// With the launch's page closed the window shows no page; an agent tab takes the size of
// the window's page area.
const launched = tabs.find(tab => tab.url === `${site.base}/open`).id;

assert.equal(text(await tool('close_tab', { tabId: launched })), 'ok');

assert.deepEqual(JSON.parse(text(await tool('list_tabs'))), [], 'the window still has tabs');

const fromEmpty = JSON.parse(text(await tool('open_tab', { url: `${site.base}/peek` })));
const emptyReady = await tool('wait_for', { tabId: fromEmpty.tabId, selector: '#marker', timeoutMs: 20000 });

assert.equal(text(emptyReady), 'ready', JSON.stringify(emptyReady));

assert.equal(text(await tool('evaluate', { tabId: fromEmpty.tabId, expression: 'innerWidth > 0 && innerHeight > 0' })), 'true');

pass('open_tab from a window showing no page loaded a page agents can drive');

// Ctrl+Q quits; the bridge must stop with the app.
execFileSync('cc', [
  '-std=c17', '-Wall', '-Wextra', '-Werror', 'spikes/gpui-cef-compositor/x11-input.c',
  '-lX11', '-lXtst', '-o', x11Driver,
]);

execFileSync(x11Driver, ['focus', 'Flecha']);

execFileSync(x11Driver, ['chord', 'Control_L', 'q']);

const [code] = await closed;

assert.equal(code, 0, `Flecha quit with ${code}`);

await assert.rejects(fetch(endpoint, { method: 'POST' }), 'the bridge still answered after Flecha quit');

pass('Ctrl+Q quit Flecha and closed the agent bridge');

site.server.close();
