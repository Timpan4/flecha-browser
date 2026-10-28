// Focused fullscreen transitions in a private browser profile and X display.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

const binary = process.argv[2];
if (!binary) throw new Error('Usage: node chromium/tests/native-fullscreen.mjs /absolute/path/to/chrome');
const profile = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-fullscreen-'));
const display = spawn('Xvfb', ['-displayfd', '3', '-screen', '0', '1280x800x24'], { stdio: ['ignore', 'ignore', 'inherit', 'pipe'] });
const number = String((await once(display.stdio[3], 'data'))[0]).trim();
const env = { ...process.env, DISPLAY: `:${number}`, HOME: profile, XDG_CONFIG_HOME: profile };
for (const key of ['WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', '__PYVENV_LAUNCHER__']) delete env[key];
const browser = spawn(binary, [`--user-data-dir=${profile}`, `--flecha-shell-dir=${path.resolve('chromium/shell/dist')}`, '--no-first-run', '--no-default-browser-check', '--disable-gpu', '--ozone-platform=x11', '--password-store=basic', '--remote-debugging-pipe'], { env, stdio: ['ignore', 'ignore', 'ignore', 'pipe', 'pipe'] });
let id = 0;
const pending = new Map();
browser.once('exit', code => {
  for (const call of pending.values()) call.reject(new Error(`Browser exited ${code}`));
  pending.clear();
});
let buffer = '';
browser.stdio[4].on('data', data => {
  buffer += data;
  const messages = buffer.split('\0');
  buffer = messages.pop();
  for (const json of messages) {
    const message = JSON.parse(json);
    const call = pending.get(message.id);
    if (!call) continue;
    pending.delete(message.id);
    if (message.error) call.reject(new Error(JSON.stringify(message.error)));
    else call.resolve(message.result);
  }
});
function send(method, params = {}, sessionId) {
  return new Promise((resolve, reject) => {
    const callId = ++id;
    pending.set(callId, { resolve, reject });
    browser.stdio[3].write(JSON.stringify({ id: callId, method, params, sessionId }) + '\0');
  });
}
async function evaluate(sessionId, expression) {
  const result = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true, userGesture: true }, sessionId);
  if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
  return result.result.value;
}
async function target(predicate) {
  for (;;) {
    const { targetInfos } = await send('Target.getTargets');
    const found = targetInfos.find(predicate);
    if (found) return (await send('Target.attachToTarget', { targetId: found.targetId, flatten: true })).sessionId;
  }
}
async function loaded(session) {
  for (;;) {
    try { if (await evaluate(session, `document.readyState === 'complete' && !!document.body`)) return; }
    catch (error) { if (!/context|navigat/i.test(error.message)) throw error; }
  }
}
async function cleanup() {
  if (browser.exitCode === null) { browser.kill('SIGTERM'); await once(browser, 'exit'); }
  if (display.exitCode === null) { display.kill('SIGTERM'); await once(display, 'exit'); }
  await fs.rm(profile, { recursive: true, force: true });
}
for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, async () => { await cleanup(); process.exit(1); });

try {
  await send('Target.setDiscoverTargets', { discover: true });
  const frame = await target(t => t.url.includes('/window.html'));
  for (;;) {
    try { if (await evaluate(frame, `document.readyState === 'complete' && !!window.flecha`)) break; }
    catch (error) { if (!/context|navigat/i.test(error.message)) throw error; }
  }
  await evaluate(frame, `globalThis.calls = new Map(); globalThis.nextCall = 0; globalThis.port = chrome.runtime.connect({ name: 'flecha' }); port.onMessage.addListener(m => { if (m.type === 'result') { const call = calls.get(m.id); calls.delete(m.id); m.error ? call?.reject(new Error(m.error)) : call?.resolve(m.value); } }); globalThis.rpc = (name, ...args) => new Promise((resolve, reject) => { const id = ++nextCall; calls.set(id, { resolve, reject }); port.postMessage({ type: 'invoke', id, name, args }); });`);
  const rpc = (name, ...args) => evaluate(frame, `rpc(${JSON.stringify(name)}, ...${JSON.stringify(args)})`);
  await rpc('finishOnboarding');
  await rpc('settings:set', { sidebarInPageFullscreen: true });
  const url = 'data:text/html,<title>Fullscreen regression</title><body style="background:teal">Fullscreen regression';
  await rpc('newTab', url);
  const page = await target(t => t.url === url);
  await loaded(page);
  const waitFrame = expression => evaluate(frame, `new Promise(resolve => { const check = () => (${expression}) ? resolve(true) : requestAnimationFrame(check); check(); })`);
  const windowState = () => evaluate(frame, 'chrome.windows.getCurrent().then(w => w.state)');
  await waitFrame(`document.body.dataset.sidebar === 'docked'`);
  await evaluate(page, 'document.documentElement.requestFullscreen()');
  assert.equal(await windowState(), 'normal');
  assert.equal(await evaluate(page, '!!document.fullscreenElement'), true);
  await rpc('toggleSidebar');
  await evaluate(frame, `new Promise(resolve => { const check = async () => (await chrome.windows.getCurrent()).state === 'fullscreen' ? resolve(true) : requestAnimationFrame(check); check(); })`);
  console.log('PASS hiding the sidebar moves page fullscreen to the native window');
  await rpc('toggleSidebar');
  await waitFrame(`document.body.dataset.sidebar === 'docked'`);
  // A window-state query is a barrier after the frame applied the docked sidebar.
  assert.equal(await windowState(), 'normal', 'restoring the sidebar must restore the native window');
  assert.equal(await evaluate(page, '!!document.fullscreenElement'), true, 'the page keeps its fullscreen element');
  await waitFrame(`!matchMedia('(display-mode: fullscreen)').matches`);
  assert.equal(await evaluate(frame, `getComputedStyle(document.querySelector('.window-controls')).display !== 'none'`), true);
  if (process.argv[3]) {
    const { data } = await send('Page.captureScreenshot', { format: 'png' }, frame);
    await fs.writeFile(process.argv[3], Buffer.from(data, 'base64'));
    await send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'dark' }] }, frame);
    const dark = await send('Page.captureScreenshot', { format: 'png' }, frame);
    await fs.writeFile(process.argv[3] + '.dark.png', Buffer.from(dark.data, 'base64'));
  }
  console.log('PASS restoring the sidebar restores window controls and keeps page fullscreen');
  await rpc('settings:set', { sidebarInPageFullscreen: false });
  await evaluate(frame, `new Promise(resolve => { const check = async () => (await chrome.windows.getCurrent()).state === 'fullscreen' ? resolve(true) : requestAnimationFrame(check); check(); })`);
  await rpc('settings:set', { sidebarInPageFullscreen: true });
  await waitFrame(`!matchMedia('(display-mode: fullscreen)').matches`);
  assert.equal(await windowState(), 'normal');
  assert.equal(await evaluate(page, '!!document.fullscreenElement'), true);
  console.log('PASS changing the sidebar fullscreen setting also restores the window');
  await evaluate(page, 'document.exitFullscreen()');
  assert.equal(await windowState(), 'normal');
  assert.equal(await evaluate(page, '!!document.fullscreenElement'), false);
  console.log('PASS the page can exit fullscreen after the sidebar returns');
  await evaluate(frame, `chrome.windows.getCurrent().then(w => chrome.windows.update(w.id, { state: 'fullscreen' }))`);
  await waitFrame(`matchMedia('(display-mode: fullscreen)').matches`);
  await evaluate(page, 'document.documentElement.requestFullscreen()');
  await rpc('toggleSidebar');
  await waitFrame(`document.body.dataset.sidebar === 'hidden'`);
  await rpc('toggleSidebar');
  await waitFrame(`document.body.dataset.sidebar === 'docked'`);
  assert.equal(await windowState(), 'fullscreen');
  assert.equal(await evaluate(page, '!!document.fullscreenElement'), true);
  await evaluate(page, 'document.exitFullscreen()');
  assert.equal(await windowState(), 'fullscreen');
  console.log('PASS explicit browser fullscreen survives restoring the sidebar and exiting page fullscreen');
} finally {
  await cleanup();
}
