// Regression checks for keys users see, record and send to the native browser.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

globalThis.chrome = {};
Object.defineProperty(globalThis, 'navigator', { value: { platform: 'Linux x86_64' }, configurable: true });
const shortcuts = await loadModule('chromium/shell/lib/shortcuts.ts');
const { shortcutMap } = await loadModule('chromium/shell/native.ts');
const { Store } = await loadModule('chromium/shared/store.ts');
const { setShortcut } = await loadModule('chromium/shell/lib/settingsCore.ts');
const store = () => new Store({ load: () => undefined, save: () => {} });

test('Linux and macOS labels and aliases match their native modifiers', () => {
  assert.equal(shortcuts.displayKeys('Alt+CmdOrCtrl+Shift+T', false), 'Ctrl+Alt+Shift+T');
  assert.equal(shortcuts.displayKeys('Alt+CmdOrCtrl+Shift+T', true), '⌥⇧⌘T');
  assert.equal(shortcuts.sameKeys('CmdOrCtrl+T', 'Ctrl+T', false), true);
  assert.equal(shortcuts.sameKeys('CmdOrCtrl+T', 'Cmd+T', false), false);
  assert.equal(shortcuts.sameKeys('CmdOrCtrl+T', 'Cmd+T', true), true);
  assert.equal(shortcuts.sameKeys('CmdOrCtrl+T', 'Ctrl+T', true), false);
});

test('default shortcuts have no native collisions on either platform', () => {
  for (const mac of [false, true]) {
    const bindings = [...Object.keys(shortcuts.fixedShortcuts(mac)), ...shortcuts.SHORTCUTS.map(({ id }) => shortcuts.keysFor(store().state, id, mac))];
    for (const [index, key] of bindings.entries()) {
      for (const other of bindings.slice(index + 1)) assert.equal(shortcuts.sameKeys(key, other, mac), false, `${key} collides with ${other}`);
    }
  }
});

test('Linux native registration separates tabs and Spaces and leaves incognito free', () => {
  const map = shortcutMap(store().state);
  assert.equal(map['CmdOrCtrl+1'], 'selectTab:1');
  assert.equal(map['Ctrl+Alt+1'], 'selectSpace:1');
  assert.equal(map['Ctrl+Shift+N'], undefined);
  assert.equal(map['Ctrl+Alt+Shift+N'], 'newSpace');
});

test('Quit is Ctrl+Shift+Q where there is no menu bar, and left to the menu bar on macOS', () => {
  assert.equal(shortcuts.keysFor(store().state, 'quit', false), 'Ctrl+Shift+Q');
  assert.equal(shortcutMap(store().state)['Ctrl+Shift+Q'], 'quit');
  assert.equal(shortcuts.keysFor(store().state, 'quit', true), '');
  assert.equal(shortcuts.keysFor(store().state, 'settings', false), 'CmdOrCtrl+,');
});

test('recording detects Control aliases, protects fixed families and resolves reset conflicts', () => {
  const s = store();
  assert.match(setShortcut(s, 'reload', 'Ctrl+T'), /New tab/);
  assert.equal(shortcuts.keysFor(s.state, 'newTab'), undefined);
  assert.equal(shortcuts.keysFor(s.state, 'reload'), 'Ctrl+T');
  for (const keys of ['Ctrl+1', 'Alt+Ctrl+9', 'Ctrl+O']) {
    assert.match(setShortcut(s, 'reload', keys), /reserved/);
    assert.equal(shortcuts.keysFor(s.state, 'reload'), 'Ctrl+T');
  }
  setShortcut(s, 'newTab', 'default');
  assert.equal(shortcuts.keysFor(s.state, 'reload'), undefined);
  assert.equal(shortcuts.keysFor(s.state, 'newTab'), 'CmdOrCtrl+T');
  assert.equal(shortcuts.shortcutList(s.state).find(({ id }) => id === 'spaceN').keys, 'Ctrl+Alt+1 … Ctrl+Alt+9');
});

test('saved custom bindings remain unambiguous when the platform changes', () => {
  const s = store();
  s.state.keybindings = { reload: 'Ctrl+T', newTab: 'Ctrl+Alt+1' };
  assert.equal(shortcuts.keysFor(s.state, 'newTab', false), undefined);
  assert.equal(shortcuts.keysFor(s.state, 'reload', false), 'Ctrl+T');
  s.state.keybindings = { reload: 'Ctrl+T' };
  assert.equal(shortcuts.keysFor(s.state, 'newTab', false), undefined);
  assert.equal(shortcuts.keysFor(s.state, 'newTab', true), 'CmdOrCtrl+T');
});
