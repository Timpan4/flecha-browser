// Settings, Tabs & Spaces on macOS keeps ⌘ for multi-select and its shortcuts. A file of its
// own: the platform is read once, when the module loads.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

Object.defineProperty(globalThis, 'navigator', { value: { platform: 'MacIntel' }, configurable: true });
const { selectionChange, paneShortcut } = await loadModule('chromium/shell/ui/tabsPane.ts');
const keys = { metaKey: false, ctrlKey: false, shiftKey: false };

test('macOS keeps ⌘-click, ⌘A and ⌘F, and leaves Control-click to the context menu', () => {
  assert.equal(selectionChange({ ...keys, metaKey: true }), 'toggle');
  assert.equal(selectionChange({ ...keys, ctrlKey: true }), 'only');
  assert.equal(paneShortcut({ ...keys, metaKey: true, key: 'a' }), 'selectAll');
  assert.equal(paneShortcut({ ...keys, ctrlKey: true, key: 'f' }), undefined);
});

test('⌘Z undoes on macOS; Control+Z does not', async () => {
  const { isUndoKey } = await loadModule('chromium/shell/lib/undo.ts');
  const z = { ...keys, altKey: false, key: 'z' };
  assert.equal(isUndoKey({ ...z, metaKey: true }), true);
  assert.equal(isUndoKey({ ...z, ctrlKey: true }), false);
});
