// The URL a pinned tab or favorite goes back to: typed like the address bar, never a search or an unsafe scheme.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { pinnableAddress } = await loadModule('chromium/shell/lib/url.ts');

test('addresses are completed like the address bar', () => {
  assert.equal(pinnableAddress('github.com'), 'https://github.com');
  assert.equal(pinnableAddress('  example.com/a?b=1 '), 'https://example.com/a?b=1');
  assert.equal(pinnableAddress('localhost:3000'), 'http://localhost:3000');
  assert.equal(pinnableAddress('https://mail.example.org/inbox'), 'https://mail.example.org/inbox');
  assert.equal(pinnableAddress('file:///home/me/notes.html'), 'file:///home/me/notes.html');
  assert.equal(pinnableAddress('about:blank'), 'about:blank');
});

test('searches, unsafe schemes and junk are refused', () => {
  for (const input of ['', '   ', 'two words', 'javascript:alert(1)', 'data:text/html,hi', 'chrome://settings', 'ftp://example.com', 'https://', undefined, null, 42]) {
    assert.equal(pinnableAddress(input), undefined, String(input));
  }
});
