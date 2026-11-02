// Exercises startup-only video settings in a temporary profile, never a user browser.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { watch } from 'node:fs';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';

const binary = process.argv[2];
const artifacts = process.argv[3];
if (!binary || !path.isAbsolute(binary)) throw new Error('Usage: node chromium/tests/native-debug-settings.mjs /absolute/path/to/chrome [artifacts]');
const shell = process.env.FLECHA_SHELL_OUT || path.resolve('chromium/shell/dist');
const profile = await fs.mkdtemp(path.join(os.tmpdir(), 'flecha-debug-settings-'));
let browser;
let display;
let displayNumber;
let waitingFor = 'Xvfb startup';
let protocolMethod;
// Reuse native-workflows.mjs's measured whole-run deadline and override.
const deadline = Number(process.env.FLECHA_HARNESS_TIMEOUT_MS) || 10 * 60 * 1000;
const watchdog = setTimeout(() => {
  console.error(`FAIL exceeded ${deadline} ms waiting for ${waitingFor}; protocol ${protocolMethod}`);
  void cleanup().finally(() => process.exit(1));
}, deadline);

async function processExited(pid) {
  // A relaunched browser is no longer Node's child. /proc also distinguishes
  // an exited zombie from a live process while its new parent reaps it.
  for (;;) {
    try {
      const stat = await fs.readFile(`/proc/${pid}/stat`, 'utf8');
      if (stat.slice(stat.lastIndexOf(')') + 2).startsWith('Z')) return;
    } catch (error) {
      if (error.code === 'ENOENT') return;
      throw error;
    }
  }
}

class Browser {
  pending = new Map();
  listeners = new Set();
  nextId = 0;

  constructor(flags) {
    const env = { ...process.env };
    delete env.__PYVENV_LAUNCHER__;
    delete env.DBUS_SESSION_BUS_ADDRESS;
    delete env.WAYLAND_DISPLAY;
    env.DISPLAY = `:${displayNumber}`;
    this.child = spawn(binary, [
      `--user-data-dir=${profile}`, `--flecha-shell-dir=${shell}`,
      '--ozone-platform=x11', '--disable-gpu', '--no-first-run', '--no-default-browser-check',
      '--disable-background-networking', '--password-store=basic', '--remote-debugging-port=0',
      ...flags,
    ], { env, detached: true, stdio: ['ignore', 'ignore', 'inherit'] });
    this.exited = once(this.child, 'exit');
    this.exited.catch(() => {});
  }

  async connect(previousEndpoint) {
    waitingFor = 'the browser DevTools endpoint';
    let observer;
    try {
      const endpoint = new Promise((resolve, reject) => {
        const check = async () => {
          try {
            const [port, route] = (await fs.readFile(path.join(profile, 'DevToolsActivePort'), 'utf8')).trim().split('\n');
            const url = `ws://127.0.0.1:${port}${route}`;
            if (route && url !== previousEndpoint) resolve(url);
          } catch (error) {
            if (error.code !== 'ENOENT') reject(error);
          }
        };
        observer = watch(profile, () => void check());
        observer.on('error', reject);
        void check();
      });
      this.endpoint = await (previousEndpoint ? endpoint : Promise.race([
        endpoint,
        this.exited.then(([code]) => { throw new Error(`Test browser exited ${code} before DevTools was ready`); }),
      ]));
    } finally {
      observer?.close();
    }
    this.socket = new WebSocket(this.endpoint);
    const socket = this.socket;
    this.closed = new Promise((resolve) => socket.addEventListener('close', resolve, { once: true }));
    socket.addEventListener('close', () => {
      for (const call of this.pending.values()) call.reject(new Error('Test browser connection closed'));
      this.pending.clear();
    });
    socket.addEventListener('message', ({ data }) => {
      const message = JSON.parse(data);
      if (message.id) {
        const call = this.pending.get(message.id);
        this.pending.delete(message.id);
        if (message.error) call?.reject(new Error(JSON.stringify(message.error)));
        else call?.resolve(message.result);
      } else {
        for (const listener of this.listeners) listener(message);
      }
    });
    await new Promise((resolve, reject) => {
      socket.addEventListener('open', resolve, { once: true });
      socket.addEventListener('error', () => reject(new Error('Test browser connection failed')), { once: true });
    });
    const { processInfo } = await this.send('SystemInfo.getProcessInfo');
    this.pid = processInfo.find(info => info.type === 'browser').id;
  }

  send(method, params = {}, sessionId) {
    protocolMethod = method;
    return new Promise((resolve, reject) => {
      if (this.socket.readyState !== WebSocket.OPEN) return reject(new Error('Test browser is disconnected'));
      const id = ++this.nextId;
      this.pending.set(id, { resolve, reject });
      this.socket.send(JSON.stringify({ id, method, params, sessionId }));
    });
  }

  async evaluate(expression) {
    const result = await this.send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, this.session);
    if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text);
    return result.result.value;
  }

  async openSettings() {
    waitingFor = 'the shell frame target';
    console.log('Test browser', (await this.send('Browser.getVersion')).product);
    await this.send('Target.setDiscoverTargets', { discover: true });
    // The frame proves the component extension is installed. Its service worker
    // may be asleep until the Settings page connects to it.
    const frame = await new Promise((resolve, reject) => {
      const accept = (target) => {
        if (target.url.startsWith('chrome-extension://') && target.url.includes('/window.html')) {
          this.listeners.delete(listener);
          resolve(target);
        }
      };
      const listener = (message) => {
        if (['Target.targetCreated', 'Target.targetInfoChanged'].includes(message.method)) accept(message.params.targetInfo);
      };
      this.listeners.add(listener);
      void this.send('Target.getTargets').then(({ targetInfos }) => targetInfos.forEach(accept), reject);
    });
    const settingsUrl = new URL('/settings.html#debug', frame.url).href;
    waitingFor = 'the Settings page to load';
    const { targetId } = await this.send('Target.createTarget', { url: settingsUrl });
    this.session = (await this.send('Target.attachToTarget', { targetId, flatten: true })).sessionId;
    await this.send('Page.enable', {}, this.session);
    await this.send('Runtime.enable', {}, this.session);
    await new Promise((resolve, reject) => {
      const check = async () => {
        try {
          if (await this.evaluate('document.readyState === "complete" && !!window.settings')) {
            this.listeners.delete(listener);
            resolve();
          }
        } catch (error) {
          if (!/context|navigat/i.test(error.message)) reject(error);
        }
      };
      const listener = (message) => {
        if (message.sessionId === this.session && ['Page.loadEventFired', 'Runtime.executionContextCreated'].includes(message.method)) void check();
      };
      this.listeners.add(listener);
      void check();
    });
    // Session restore can select another tab while this page loads. Activate
    // only this test browser's tab so visibility-gated settings reads can run.
    await this.send('Page.bringToFront', {}, this.session);
  }

  waitStatus(text) {
    waitingFor = `Debug status: ${text}`;
    return this.evaluate(`new Promise(resolve => {
      const status = document.getElementById('debug-video-buffer-status');
      const check = () => { if (status.textContent.includes(${JSON.stringify(text)})) { observer.disconnect(); resolve(); } };
      const observer = new MutationObserver(check);
      observer.observe(status, { childList: true, characterData: true, subtree: true });
      check();
    })`);
  }

  async screenshot(theme) {
    if (!artifacts) return;
    waitingFor = `the ${theme} screenshot`;
    await this.send('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: theme }] }, this.session);
    const { data } = await this.send('Page.captureScreenshot', { format: 'png' }, this.session);
    await fs.writeFile(path.join(artifacts, `debug-settings-${theme}.png`), Buffer.from(data, 'base64'));
  }

  async close() {
    waitingFor = 'the test browser to close';
    await this.send('Browser.close').catch(() => {});
    await this.closed;
    await processExited(this.pid);
    this.stop();
  }

  stop() {
    // The Linux relaunch implementation inherits the process group created by
    // this harness. Killing that group also covers a relaunch before reconnect.
    if (!this.child.pid) return;
    try { process.kill(-this.child.pid, 'SIGKILL'); }
    catch (error) { if (error.code !== 'ESRCH') throw error; }
  }

  async restart() {
    waitingFor = 'chrome://restart to relaunch the browser';
    const socket = this.socket;
    await Promise.all([
      this.closed,
      this.send('Target.createTarget', { url: 'chrome://restart' }).catch(error => {
        if (socket.readyState === WebSocket.OPEN) throw error;
      }),
    ]);
    this.listeners.clear();
    await this.connect(this.endpoint);
    await this.openSettings();
  }
}

async function cleanup() {
  browser?.stop();
  if (display && display.exitCode === null && display.signalCode === null) {
    const exited = once(display, 'exit');
    display.kill('SIGKILL');
    await exited;
  }
  await fs.rm(profile, { recursive: true, force: true });
}
for (const signal of ['SIGINT', 'SIGTERM']) process.once(signal, () => {
  void cleanup().finally(() => process.exit(1));
});

async function start(...flags) {
  await fs.rm(path.join(profile, 'DevToolsActivePort'), { force: true });
  browser = new Browser(flags);
  await browser.connect();
  await browser.openSettings();
}
const state = () => browser.evaluate('window.settings.getDebugSettings()');
const click = () => {
  waitingFor = 'the Debug toggle to become usable';
  return browser.evaluate(`new Promise(resolve => {
  const toggle = document.getElementById('debug-video-buffer');
  const check = () => {
    if (!toggle.disabled) { observer.disconnect(); toggle.click(); resolve(); }
  };
  const observer = new MutationObserver(check);
  observer.observe(toggle, { attributes: true, attributeFilter: ['disabled'] });
  check();
})`);
};

try {
  display = spawn('Xvfb', ['-displayfd', '3', '-screen', '0', '1280x800x24'], { stdio: ['ignore', 'ignore', 'inherit', 'pipe'] });
  displayNumber = await Promise.race([
    once(display.stdio[3], 'data').then(([data]) => String(data).trim()),
    once(display, 'exit').then(([code]) => { throw new Error(`Xvfb exited ${code} before it was ready`); }),
  ]);
  if (artifacts) await fs.mkdir(artifacts, { recursive: true });
  await start();
  assert.deepEqual(await state(), { disableGpuMemoryBufferVideoFrames: false, videoBufferWorkaroundActive: false, videoBufferWorkaroundForced: false });
  await browser.waitStatus('off in this browser session');
  await browser.screenshot('light');
  await browser.screenshot('dark');
  await click();
  await browser.waitStatus('Restart Flecha to turn the workaround on');
  assert.deepEqual(await state(), { disableGpuMemoryBufferVideoFrames: true, videoBufferWorkaroundActive: false, videoBufferWorkaroundForced: false });
  await browser.close();
  console.log('PASS saves from the Debug pane without changing the running video mode');

  await start();
  assert.deepEqual(await state(), { disableGpuMemoryBufferVideoFrames: true, videoBufferWorkaroundActive: true, videoBufferWorkaroundForced: false });
  await browser.waitStatus('on in this browser session');
  await browser.restart();
  assert.deepEqual(await state(), { disableGpuMemoryBufferVideoFrames: true, videoBufferWorkaroundActive: true, videoBufferWorkaroundForced: false });
  await browser.waitStatus('on in this browser session');
  console.log('PASS browser relaunch preserves the preference without turning it into a launcher override');
  await click();
  await browser.waitStatus('Restart Flecha to turn the workaround off');
  await browser.restart();
  assert.deepEqual(await state(), { disableGpuMemoryBufferVideoFrames: false, videoBufferWorkaroundActive: false, videoBufferWorkaroundForced: false });
  await browser.waitStatus('off in this browser session');
  await browser.close();
  console.log('PASS saved preference takes effect on restart and can be turned off');

  await start();
  assert.deepEqual(await state(), { disableGpuMemoryBufferVideoFrames: false, videoBufferWorkaroundActive: false, videoBufferWorkaroundForced: false });
  await browser.close();
  console.log('PASS restarting after disabling restores the default');

  await start('--disable-gpu-memory-buffer-video-frames');
  await browser.waitStatus('Enabled by a command-line flag');
  assert.equal(await browser.evaluate('document.getElementById("debug-video-buffer").disabled'), true);
  assert.equal((await state()).videoBufferWorkaroundForced, true);
  assert.equal(await browser.evaluate('window.settings.setVideoBufferWorkaround(false).then(() => false, () => true)'), true);
  await browser.restart();
  await browser.waitStatus('Enabled by a command-line flag');
  assert.equal((await state()).videoBufferWorkaroundForced, true);
  await browser.close();
  console.log('PASS explicit command-line overrides are shown and protected');
} finally {
  await cleanup();
  clearTimeout(watchdog);
}
