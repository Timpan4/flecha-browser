// Text fields commit on Enter, but not on the Enter that picks an IME candidate.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { isEnter } = await loadModule('chromium/shell/lib/keys.ts');

test('Enter commits, except while an IME (Japanese, Chinese, Korean) is composing', () => {
  assert.equal(isEnter({ key: 'Enter', isComposing: false }), true);
  assert.equal(isEnter({ key: 'Enter', isComposing: true }), false);
  assert.equal(isEnter({ key: 'a', isComposing: false }), false);
});
