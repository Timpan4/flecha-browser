// The Settings control preserves the running video mode until the next start.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { setImmediate } from 'node:timers/promises';
import { loadModule } from './load-module.mjs';

const { setupDebugPane } = await loadModule('chromium/shell/ui/debugPane.ts');
const initial = {
  disableGpuMemoryBufferVideoFrames: false,
  videoBufferWorkaroundActive: false,
  videoBufferWorkaroundForced: false,
};

function mount(t, api) {
  const pane = { hidden: false };
  const toggle = Object.assign(new EventTarget(), { checked: false, disabled: true });
  const status = { textContent: '' };
  globalThis.document = Object.assign(new EventTarget(), {
    hidden: false,
    querySelector: () => pane,
    getElementById: (id) => id === 'debug-video-buffer' ? toggle : status,
  });
  globalThis.window = new EventTarget();
  globalThis.MutationObserver = class {
    observe() {}
    disconnect() {}
  };
  setupDebugPane(api);
  t.after(() => window.dispatchEvent(new Event('pagehide')));
  return { toggle, status };
}

test('saving the workaround changes the next start, persists on reopening, and can be reverted', async (t) => {
  let settings = { ...initial };
  const writes = [];
  const api = {
    getDebugSettings: async () => settings,
    setVideoBufferWorkaround: async (enabled) => {
      writes.push(enabled);
      settings = { ...settings, disableGpuMemoryBufferVideoFrames: enabled };
      return settings;
    },
  };
  const { toggle, status } = mount(t, api);
  await setImmediate();
  assert.equal(toggle.checked, false);
  assert.equal(toggle.disabled, false);
  assert.match(status.textContent, /off in this browser session/);
  assert.deepEqual(writes, []);

  toggle.checked = true;
  toggle.dispatchEvent(new Event('change'));
  assert.equal(toggle.disabled, true);
  await setImmediate();
  assert.deepEqual(writes, [true]);
  assert.equal(toggle.checked, true);
  assert.equal(settings.videoBufferWorkaroundActive, false);
  assert.match(status.textContent, /Restart Flecha.*on/);

  window.dispatchEvent(new Event('focus'));
  await setImmediate();
  assert.equal(toggle.checked, true);
  assert.match(status.textContent, /Restart Flecha.*on/);

  toggle.checked = false;
  toggle.dispatchEvent(new Event('change'));
  await setImmediate();
  assert.deepEqual(writes, [true, false]);
  assert.match(status.textContent, /off in this browser session/);

  // A new browser started with the saved preference on.
  settings = { ...initial, disableGpuMemoryBufferVideoFrames: true, videoBufferWorkaroundActive: true };
  window.dispatchEvent(new Event('focus'));
  await setImmediate();
  assert.match(status.textContent, /on in this browser session/);
  toggle.checked = false;
  toggle.dispatchEvent(new Event('change'));
  await setImmediate();
  assert.match(status.textContent, /Restart Flecha.*off/);
});

test('a failed save restores the saved checkbox and reports the failure', async (t) => {
  const { toggle, status } = mount(t, {
    getDebugSettings: async () => initial,
    setVideoBufferWorkaround: async () => { throw new Error('Write failed'); },
  });
  await setImmediate();
  toggle.checked = true;
  toggle.dispatchEvent(new Event('change'));
  await setImmediate();
  assert.equal(toggle.checked, false);
  assert.equal(toggle.disabled, false);
  assert.match(status.textContent, /Could not save:.*Write failed/);
});

test('a command-line override stays checked and cannot be changed in Settings', async (t) => {
  let writes = 0;
  const { toggle, status } = mount(t, {
    getDebugSettings: async () => ({ ...initial, videoBufferWorkaroundActive: true, videoBufferWorkaroundForced: true }),
    setVideoBufferWorkaround: async () => { writes++; return initial; },
  });
  await setImmediate();
  assert.equal(toggle.checked, true);
  assert.equal(toggle.disabled, true);
  assert.match(status.textContent, /Enabled by a command-line flag/);
  toggle.dispatchEvent(new Event('change'));
  await setImmediate();
  assert.equal(writes, 0);
});

test('a failed refresh preserves the usable saved setting', async (t) => {
  let reads = 0;
  const { toggle, status } = mount(t, {
    getDebugSettings: async () => {
      if (++reads > 1) throw new Error('Read failed');
      return initial;
    },
    setVideoBufferWorkaround: async (enabled) => ({ ...initial, disableGpuMemoryBufferVideoFrames: enabled }),
  });
  await setImmediate();
  window.dispatchEvent(new Event('focus'));
  await setImmediate();
  assert.equal(toggle.disabled, false);
  assert.equal(toggle.checked, false);
  assert.match(status.textContent, /Read failed/);
  toggle.checked = true;
  toggle.dispatchEvent(new Event('change'));
  await setImmediate();
  assert.equal(toggle.checked, true);
  assert.match(status.textContent, /Restart Flecha.*on/);
});

test('older native builds keep the control disabled and explain why', async (t) => {
  const { toggle, status } = mount(t, {});
  await setImmediate();
  assert.equal(toggle.disabled, true);
  assert.match(status.textContent, /unavailable in this browser build/);
});

test('a stale refresh cannot overwrite a newer saved preference', async (t) => {
  let resolveRead;
  let reads = 0;
  const { toggle, status } = mount(t, {
    getDebugSettings: () => ++reads === 1 ? Promise.resolve(initial) : new Promise((resolve) => { resolveRead = resolve; }),
    setVideoBufferWorkaround: async () => ({ ...initial, disableGpuMemoryBufferVideoFrames: true }),
  });
  await setImmediate();
  window.dispatchEvent(new Event('focus'));
  toggle.checked = true;
  toggle.dispatchEvent(new Event('change'));
  await setImmediate();
  resolveRead(initial);
  await setImmediate();
  assert.equal(toggle.checked, true);
  assert.match(status.textContent, /Restart Flecha.*on/);
});
