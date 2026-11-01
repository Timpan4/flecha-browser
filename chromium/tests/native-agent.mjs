// A coding agent works in the background: it drives its own tab over MCP while you keep typing in
// yours. Runs the compiled browser on a private X display and profile, like native-workflows.mjs,
// and never attaches to a user browser.
//
//   node chromium/tests/native-agent.mjs /absolute/path/to/chrome [/absolute/artifacts/dir]
import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { createServer } from 'node:http';
import { once } from 'node:events';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';

const binary = process.argv[2];
const artifacts = process.argv[3];
if (!binary) throw new Error('Usage: node chromium/tests/native-agent.mjs /absolute/path/to/chrome [/absolute/artifacts/dir]');
const shellDir = process.env.FLECHA_SHELL_OUT || path.resolve('chromium/shell/dist');
const profile = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-agent-'));
const inputTool = path.join(profile, 'x11-input');
execFileSync('c++', ['chromium/tests/x11-input.cc', '-o', inputTool, '-lX11', '-lXtst'], { stdio: 'inherit' });
const display = spawn('Xvfb', ['-displayfd', '3', '-screen', '0', '1280x800x24'], { stdio: ['ignore', 'ignore', 'inherit', 'pipe'] });
const displayNumber = String((await once(display.stdio[3], 'data'))[0]).trim();
const env = { ...process.env, DISPLAY: `:${displayNumber}` };
for (const name of ['__PYVENV_LAUNCHER__', 'DBUS_SESSION_BUS_ADDRESS', 'WAYLAND_DISPLAY']) delete env[name];
env.HOME = path.join(profile, 'home');
for (const name of ['XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME']) env[name] = path.join(profile, name.toLowerCase());

// Your page, with a draft you type into, and the agent's, which counts clicks and logs keys.
const server = createServer((request, response) => {
  response.setHeader('Content-Type', 'text/html');
  if (request.url === '/mine') return response.end('<title>My page</title><textarea id="draft" style="position: fixed; inset: 0; width: 100%; height: 100%"></textarea>');
  response.end(
    '<title>Agent page</title><h1>Agent page</h1><input id="field"><button id="count" onclick="counter++">Count</button>' +
      '<script>window.counter = 0; window.keys = []; addEventListener("keydown", (e) => keys.push(e.key));</script>',
  );
});
server.listen(0, '127.0.0.1');
await once(server, 'listening');
const base = `http://127.0.0.1:${server.address().port}`;

let browser;
let id = 0;
const pending = new Map();
const listeners = new Set();
let finished = false;
/** What the run waits for now; the deadline names it. */
let waitingFor;
/** What the deadline's failure prints about your page and the X focus, once there is a page. */
let diagnose;
const deadline = Number(process.env.FLECHA_HARNESS_TIMEOUT_MS) || 5 * 60 * 1000;
/** Stops a child that is still running; one that exited, or died from a signal, has nothing to wait for. */
async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill('SIGTERM');
  await once(child, 'exit');
}
async function cleanUp() {
  if (browser) await stop(browser);
  await stop(display);
  server.close();
  await fs.rm(profile, { recursive: true, force: true });
}
const watchdog = setTimeout(async () => {
  if (finished) return;
  // The X focus is read now. The page may not answer; stopping the browser ends its call, so
  // the diagnostic never holds up the cleanup.
  let diagnostic;
  try {
    diagnostic = diagnose?.();
  } catch (error) {
    diagnostic = { error: error.message };
  }
  console.error(`FAIL the run took longer than ${deadline} ms (FLECHA_HARNESS_TIMEOUT_MS); the last step is the last PASS above, waiting for ${waitingFor}`);
  await cleanUp();
  if (diagnostic) console.error('DIAGNOSTIC', { ...diagnostic, page: await diagnostic.page });
  process.exit(1);
}, deadline);
for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, async () => { await cleanUp(); process.exit(1); });

function send(method, params = {}, sessionId) {
  return new Promise((resolve, reject) => {
    const callId = ++id;
    pending.set(callId, { resolve, reject });
    browser.stdio[3].write(JSON.stringify({ id: callId, method, params, sessionId }) + '\0');
  });
}
async function evaluate(session, expression) {
  const result = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, session);
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
  return result.result.value;
}
const attach = async (targetId) => (await send('Target.attachToTarget', { targetId, flatten: true })).sessionId;
function findTarget(predicate) {
  waitingFor = `a target where ${predicate}`;
  return new Promise((resolve, reject) => {
    const accept = (target) => {
      if (predicate(target)) {
        listeners.delete(listener);
        resolve(target);
      }
    };
    const listener = (message) => {
      if (message.method === 'Target.targetCreated' || message.method === 'Target.targetInfoChanged') accept(message.params.targetInfo);
    };
    listeners.add(listener);
    void send('Target.getTargets').then(({ targetInfos }) => targetInfos.forEach(accept), reject);
  });
}
async function loaded(session, urlPrefix) {
  waitingFor = `${urlPrefix} to load`;
  await send('Page.enable', {}, session);
  await send('Runtime.enable', {}, session);
  await new Promise((resolve, reject) => {
    const check = async () => {
      try {
        if (await evaluate(session, `location.href.startsWith(${JSON.stringify(urlPrefix)}) && document.readyState === 'complete'`)) {
          listeners.delete(listener);
          resolve();
        }
      } catch (error) {
        if (!/context|navigat/i.test(error.message)) reject(error);
      }
    };
    const listener = (message) => {
      if (message.sessionId === session && ['Page.loadEventFired', 'Runtime.executionContextCreated'].includes(message.method)) void check();
    };
    listeners.add(listener);
    void check();
  });
}

try {
  // The helper is there from the start: the bridge runs in it.
  await fs.mkdir(profile, { recursive: true });
  execFileSync(process.execPath, ['chromium/scripts/install-native-host.mjs', '--profile', profile], { env, stdio: 'inherit' });
  browser = spawn(
    binary,
    [`--user-data-dir=${profile}`, `--flecha-shell-dir=${shellDir}`, '--no-first-run', '--no-default-browser-check', '--disable-gpu', '--ozone-platform=x11', '--password-store=basic', '--remote-debugging-pipe'],
    { env, stdio: ['ignore', 'ignore', 'inherit', 'pipe', 'pipe'] },
  );
  browser.once('exit', (code) => {
    for (const call of pending.values()) call.reject(new Error(`Browser exited ${code}`));
    pending.clear();
  });
  let buffer = '';
  browser.stdio[4].on('data', (data) => {
    buffer += data.toString();
    const messages = buffer.split('\0');
    buffer = messages.pop();
    for (const json of messages) {
      const message = JSON.parse(json);
      if (message.id) {
        const call = pending.get(message.id);
        pending.delete(message.id);
        if (message.error) call?.reject(new Error(JSON.stringify(message.error)));
        else call?.resolve(message.result);
      } else for (const listener of listeners) listener(message);
    }
  });
  console.log('Browser', (await send('Browser.getVersion')).product);
  await send('Target.setDiscoverTargets', { discover: true });
  const extension = 'chrome-extension://abdiaepfgbbhdjaecjipmaailijaaide/';
  const session = await attach((await findTarget(({ url }) => url.startsWith(extension + 'window.html'))).targetId);
  await loaded(session, extension);
  await evaluate(session, `
    globalThis.testPort = chrome.runtime.connect({name: 'flecha'});
    globalThis.testCalls = new Map(); globalThis.testWaiters = new Set(); globalThis.testId = 0;
    testPort.onMessage.addListener(m => {
      if (m.type === 'result') { const call = testCalls.get(m.id); testCalls.delete(m.id); m.error ? call?.reject(new Error(m.error)) : call?.resolve(m.value); }
      if (m.type === 'event' && m.name === 'state') { globalThis.testState = m.args[0]; for (const check of testWaiters) check(); }
    });
    globalThis.testInvoke = (name, ...args) => new Promise((resolve,reject) => { const id = ++testId; testCalls.set(id,{resolve,reject}); testPort.postMessage({type:'invoke',id,name,args}); });
    globalThis.testWait = predicate => new Promise(resolve => { const check = () => { if (globalThis.testState && predicate(testState)) { testWaiters.delete(check); resolve(testState); } }; testWaiters.add(check); check(); });
    testPort.postMessage({type:'cmd',name:'ready',args:[]});
  `);
  const rpc = (name, ...args) => evaluate(session, `testInvoke(${JSON.stringify(name)}, ...${JSON.stringify(args)})`);
  const stateWhere = (predicate) => ((waitingFor = `state where ${predicate}`), evaluate(session, `testWait(${predicate})`));
  const domWhere = (predicate) =>
    ((waitingFor = `sidebar where ${predicate}`), evaluate(session, `new Promise(resolve => { const check = () => { if ((${predicate})()) { observer.disconnect(); resolve(true); } }; const observer = new MutationObserver(check); observer.observe(document.body, {subtree:true,childList:true,attributes:true,characterData:true}); check(); })`));
  await stateWhere('s => s.spaces.length > 0');
  await rpc('finishOnboarding');
  await stateWhere('s => s.onboarded');

  // You are typing in your own tab.
  await rpc('newTab', base + '/mine');
  let state = await stateWhere(`s => Object.values(s.tabs).some(t => t.url === ${JSON.stringify(base + '/mine')})`);
  const mine = Object.values(state.tabs).find((t) => t.url === base + '/mine').id;
  const myPage = await attach((await findTarget((t) => t.type === 'page' && t.url === base + '/mine')).targetId);
  await loaded(myPage, base + '/mine');
  const x11 = (...args) => execFileSync(inputTool, [env.DISPLAY, String(browser.pid), ...args], { env });
  diagnose = () => ({
    xFocus: String(x11('focus')).trim(),
    windows: String(x11('windows')).trim(),
    page: evaluate(myPage, "({ draft: document.querySelector('#draft').value, active: document.activeElement?.id, hasFocus: document.hasFocus(), visibility: document.visibilityState })").catch((error) => error.message),
  });
  /** Waits until your draft holds `text`, as each key you type reaches it. */
  const draftIs = (text) => {
    waitingFor = `your draft to read ${JSON.stringify(text)}`;
    return evaluate(myPage, `new Promise(resolve => { const draft = document.querySelector('#draft'); const check = () => draft.value === ${JSON.stringify(text)} && resolve(); draft.addEventListener('input', check); check(); })`);
  };
  // The draft has focus in your page. A Shift press gives the window the X focus, as your click
  // would. Chromium can still drop the first key after it takes the focus, so a key is pressed until
  // one lands, checked every 50 ms as native-workflows.mjs waits, and the draft is emptied again.
  await evaluate(myPage, "document.querySelector('#draft').focus()");
  x11('Shift_L');
  waitingFor = 'Chromium to take the window focus';
  await evaluate(
    session,
    "chrome.windows.getAll().then((all) => all.some((w) => w.focused) || new Promise((resolve) => chrome.windows.onFocusChanged.addListener(function focused(id) { if (id !== chrome.windows.WINDOW_ID_NONE) { chrome.windows.onFocusChanged.removeListener(focused); resolve(true); } })))",
  );
  waitingFor = 'a first key to reach your draft';
  while (!(await evaluate(myPage, "document.querySelector('#draft').value"))) {
    x11('type', 'x');
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  await evaluate(myPage, "document.querySelector('#draft').value = ''");
  x11('type', 'ab');
  await draftIs('ab');
  console.log('PASS your keystrokes reach your page before the agent starts');

  // The agent connects over MCP with the token from Settings.
  await rpc('settings:set', { agentEnabled: true });
  let view = await rpc('settings:get');
  while (!view.agent.endpoint) {
    assert.ok(!view.hostError, view.hostError);
    await new Promise((resolve) => setTimeout(resolve, 100));
    view = await rpc('settings:get');
  }
  const client = new Client({ name: 'flecha-agent-test', version: '1.0.0' });
  await client.connect(new StreamableHTTPClientTransport(new URL(view.agent.endpoint), { requestInit: { headers: { Authorization: `Bearer ${view.agent.token}` } } }));
  const call = async (name, args = {}) => {
    const result = await client.callTool({ name, arguments: args });
    assert.ok(!result.isError, `${name} failed: ${result.content?.[0]?.text}`);
    return result;
  };
  const yours = (label) =>
    stateWhere(`s => s.spaces.find(sp => sp.id === s.activeSpaceId).activeTabId === ${JSON.stringify(mine)}`).then(() => console.log(`PASS your tab stays on screen after ${label}`));

  const opened = JSON.parse((await call('open_tab', { url: base + '/agent' })).content[0].text);
  state = await stateWhere(`s => s.tabs[${JSON.stringify(opened.tabId)}]?.agent`);
  await yours('open_tab');
  await domWhere(`() => !document.querySelector('#agent-folder').hidden && document.querySelector('#agent-folder').textContent.includes('Agent page')`);
  assert.equal(await evaluate(session, "[...document.querySelectorAll('#today .title')].some(t => t.textContent === 'Agent page')"), false);
  console.log("PASS the agent's tab opens in the background, in the agent's folder and not among Today's tabs");
  if (artifacts) {
    await fs.mkdir(artifacts, { recursive: true });
    for (const scheme of ['light', 'dark']) {
      await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: scheme }] }, session);
      await fs.writeFile(path.join(artifacts, `agent-folder-${scheme}.png`), Buffer.from((await send('Page.captureScreenshot', { format: 'png' }, session)).data, 'base64'));
    }
    await send('Emulation.setEmulatedMedia', { features: [] }, session);
  }
  const busyTitle = await evaluate(session, "document.querySelector('#agent-folder .folder-name').textContent");
  assert.equal(busyTitle, 'Agent is working…');
  console.log('PASS the folder says the agent is working while it works');

  const agentPage = await attach((await findTarget((t) => t.type === 'page' && t.url === base + '/agent')).targetId);
  await loaded(agentPage, base + '/agent');
  assert.equal(await evaluate(agentPage, 'document.visibilityState'), 'hidden');

  // A page tool names its tab: several agents share the bridge, so there is no default tab to guess.
  const unnamed = await client.callTool({ name: 'click', arguments: { selector: '#count' } });
  assert.equal(unnamed.isError, true);
  console.log('PASS a page tool without a tab id is refused');
  const agentTab = opened.tabId;
  await call('click', { tabId: agentTab, selector: '#count' });
  await yours('click');
  // You keep typing while the agent types into its own page.
  const typing = call('type', { tabId: agentTab, selector: '#field', text: 'from the agent', submit: true });
  x11('type', 'cd');
  await typing;
  await yours('type');
  await call('press_key', { tabId: agentTab, key: 'Escape' });
  await yours('press_key');
  const shot = await call('screenshot', { tabId: agentTab });
  assert.equal(shot.content[0].type, 'image');
  assert.ok(shot.content[0].data.length > 1000);
  await yours('screenshot');
  const read = await call('read_page', { tabId: agentTab });
  assert.match(read.content[0].text, /Agent page/);
  assert.deepEqual(
    await evaluate(agentPage, "({ counter, field: document.querySelector('#field').value, keys: keys.filter(k => k === 'Enter' || k === 'Escape') })"),
    { counter: 1, field: 'from the agent', keys: ['Enter', 'Escape'] },
  );
  console.log("PASS click, type, press_key, screenshot and read_page work in the agent's tab while it is hidden");
  x11('type', 'e');
  await draftIs('abcde');
  assert.equal(await evaluate(myPage, "document.activeElement.id + ' ' + document.hasFocus() + ' ' + document.visibilityState"), 'draft true visible');
  console.log('PASS your keystrokes kept reaching your draft while the agent worked');
  if (artifacts) await fs.writeFile(path.join(artifacts, 'agent-screenshot.png'), Buffer.from(shot.content[0].data, 'base64'));

  // Tab sleep may discard an agent tab the bridge isn't attached to: one it opened and hasn't used
  // yet, or one it let go of after a quiet while (nativeHost.ts DEBUG_IDLE). Chromium won't discard
  // a tab with a debugger attached or with form input. The agent's next call brings the page back, hidden.
  const idle = JSON.parse((await call('open_tab', { url: base + '/idle' })).content[0].text).tabId;
  const discarded = await evaluate(
    session,
    `chrome.tabs.query({}).then(async (tabs) => {
      const tabId = tabs.find((t) => t.url === ${JSON.stringify(base + '/idle')}).id;
      return (await chrome.flechaPrivate.discardTab(tabId)) && (await chrome.tabs.get(tabId)).discarded;
    })`,
  );
  assert.equal(discarded, true, 'Chromium discarded the idle agent tab');
  const heading = await call('evaluate', { tabId: idle, expression: "document.querySelector('h1')?.textContent ?? null" });
  assert.equal(heading.content[0].text, 'Agent page');
  await yours('a call on a discarded tab');
  console.log("PASS a call on the agent's discarded tab brings its page back, in the background");
  await call('close_tab', { tabId: idle });
  await stateWhere(`s => !s.tabs[${JSON.stringify(idle)}]`);

  // A tab you pin is yours: it leaves the agent's folder.
  await rpc('togglePin', opened.tabId);
  await stateWhere(`s => !s.tabs[${JSON.stringify(opened.tabId)}].agent`);
  await domWhere("() => document.querySelector('#agent-folder').hidden");
  console.log("PASS pinning the agent's tab takes it out of the agent's folder");

  await client.close();
  finished = true;
  clearTimeout(watchdog);
  await cleanUp();
} catch (error) {
  console.error('FAIL', error);
  finished = true;
  clearTimeout(watchdog);
  await cleanUp();
  process.exit(1);
}
