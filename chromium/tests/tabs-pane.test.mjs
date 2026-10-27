// Settings, Tabs & Spaces off macOS: multi-select and its shortcuts use Ctrl, not the Super key.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

Object.defineProperty(globalThis, 'navigator', { value: { platform: 'Linux x86_64' }, configurable: true });
const { selectionChange, paneShortcut } = await loadModule('chromium/shell/ui/tabsPane.ts');
const keys = { metaKey: false, ctrlKey: false, shiftKey: false };

test('Ctrl-click adds a row to the selection on Linux and Windows; the Super key does not', () => {
  assert.equal(selectionChange({ ...keys, ctrlKey: true }), 'toggle');
  assert.equal(selectionChange({ ...keys, metaKey: true }), 'only');
  assert.equal(selectionChange({ ...keys, shiftKey: true }), 'range');
  assert.equal(selectionChange(keys), 'only');
});

test('Ctrl+A and Ctrl+F select every tab and find on Linux and Windows', () => {
  assert.equal(paneShortcut({ ...keys, ctrlKey: true, key: 'a' }), 'selectAll');
  assert.equal(paneShortcut({ ...keys, ctrlKey: true, key: 'f' }), 'find');
  assert.equal(paneShortcut({ ...keys, metaKey: true, key: 'a' }), undefined);
});

test('Ctrl+Z undoes on Linux and Windows; Super+Z, Ctrl+Shift+Z and Ctrl+Alt+Z do not', async () => {
  const { isUndoKey } = await loadModule('chromium/shell/lib/undo.ts');
  const z = { ...keys, altKey: false, key: 'z' };
  assert.equal(isUndoKey({ ...z, ctrlKey: true }), true);
  assert.equal(isUndoKey({ ...z, ctrlKey: true, key: 'Z' }), true);
  assert.equal(isUndoKey({ ...z, metaKey: true }), false);
  assert.equal(isUndoKey({ ...z, ctrlKey: true, shiftKey: true }), false);
  assert.equal(isUndoKey({ ...z, ctrlKey: true, altKey: true }), false);
});
