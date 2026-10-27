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
