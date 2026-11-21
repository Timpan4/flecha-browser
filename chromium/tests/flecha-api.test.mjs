import assert from 'node:assert/strict';
import { test } from 'node:test';
import { setImmediate } from 'node:timers/promises';
import { loadModule } from './load-module.mjs';

test('the toolbar overrides the surface and only state consumers use the sidebar port', async t => {
  const connections = [];
  globalThis.location = { pathname: '/window.html' };
  globalThis.document = { documentElement: { dataset: { surface: 'window' } } };
  globalThis.chrome = { runtime: { connect({ name }) {
    connections.push(name);
    return { onMessage: { addListener() {} } };
  } } };
  t.mock.method(globalThis, 'setInterval', () => {});
  const { portForPage } = await loadModule('chromium/shell/client.ts');
  assert.deepEqual(connections, ['flecha-sidebar']);
  for (const [path, surface, expected] of [
    ['/flechinha.html', 'window', 'flecha-toolbar'],
    ['/sidebar.html', 'panel', 'flecha-sidebar'],
    ['/newtab.html', 'newtab', 'flecha-sidebar'],
    ['/settings.html', undefined, 'flecha-settings'],
    ['/history.html', undefined, 'flecha-page'],
  ]) assert.equal(portForPage(path, surface), expected);
});

test('native frame and fullscreen failures retry without dropping newer requests', async t => {
  const calls = { frame: [], fullscreen: [] };
  const request = (kind, value) => {
    const response = Promise.withResolvers();
    calls[kind].push({ value, ...response });
    return response.promise;
  };
  let receive;
  globalThis.location = { pathname: '/window.html' };
  globalThis.document = { documentElement: { dataset: { surface: 'window' }, style: { setProperty() {} } } };
  globalThis.window = { addEventListener() {} };
  globalThis.matchMedia = () => ({ matches: false, addEventListener() {} });
  globalThis.chrome = {
    runtime: { connect: () => ({ onMessage: { addListener: listener => { receive = listener; } } }) },
    flechaPrivate: {
      getWindowId: async () => 41,
      setFrame: value => request('frame', value.overlay),
      setSidebarInPageFullscreen: value => request('fullscreen', value),
    },
  };
  t.mock.method(globalThis, 'setInterval', () => {});
  await loadModule('chromium/shell/flechaApi.ts');
  const emit = (name, value) => receive({ type: 'event', name, args: [value] });
  const frame = (enabled) => emit('frame', { sidebar: 'docked', overlay: enabled, sidebarInPageFullscreen: enabled });
  emit('state', { tint: '#ffffff', sidebarWidth: 260 });
  frame(false);
  await setImmediate();
  for (const kind of Object.values(calls)) {
    assert.deepEqual(kind.map(call => call.value), [false]);
    kind[0].reject(new Error('Native frame host unavailable'));
  }
  await setImmediate();

  frame(false); // Retry the same values, then cycle back to them while the retry is pending.
  frame(true);
  frame(false);
  await setImmediate();
  for (const kind of Object.values(calls)) {
    assert.deepEqual(kind.map(call => call.value), [false, false, true, false]);
    kind[1].reject(new Error('Older request with the same value failed'));
    kind[2].reject(new Error('Older request with a different value failed'));
  }
  await setImmediate();
  frame(false);
  await setImmediate();
  assert.equal(calls.frame.length, 4);
  assert.equal(calls.fullscreen.length, 4);

  calls.frame[3].reject(new Error('Current frame request failed'));
  calls.fullscreen[3].resolve();
  await setImmediate();
  frame(false);
  await setImmediate();
  assert.equal(calls.frame[4]?.value, false);
  assert.equal(calls.fullscreen.length, 4);
  calls.frame[4].resolve();
});
