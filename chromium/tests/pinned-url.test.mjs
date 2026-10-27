// The URL a pinned tab or favorite goes back to: typed like the address bar, never a search or an unsafe scheme.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { pinnableAddress, sameAddress } = await loadModule('chromium/shell/lib/url.ts');

test('addresses are completed like the address bar, written as the tab reports its page', () => {
  assert.equal(pinnableAddress('github.com'), 'https://github.com/');
  assert.equal(pinnableAddress('  Example.COM/a?b=1 '), 'https://example.com/a?b=1');
  assert.equal(pinnableAddress('localhost:3000'), 'http://localhost:3000/');
  assert.equal(pinnableAddress('https://mail.example.org/inbox'), 'https://mail.example.org/inbox');
  assert.equal(pinnableAddress('about:blank'), 'about:blank');
});

test('searches, pages the shell cannot open or keep, and junk are refused', () => {
  for (const input of ['', '   ', 'two words', 'javascript:alert(1)', 'data:text/html,hi', 'chrome://settings', 'about:version', 'about:settings', 'file:///home/me/notes.html', 'ftp://example.com', 'https://', undefined, null, 42]) {
    assert.equal(pinnableAddress(input), undefined, String(input));
  }
});

test('a saved address matches its page however either is written', () => {
  assert.equal(sameAddress('http://127.0.0.1:43067', 'http://127.0.0.1:43067/'), true);
  assert.equal(sameAddress('https://EXAMPLE.com', 'https://example.com/'), true);
  assert.equal(sameAddress('https://example.com/a', 'https://example.com/b'), false);
  assert.equal(sameAddress('chrome://settings', 'chrome://settings'), true);
  assert.equal(sameAddress('chrome://settings', 'chrome://history'), false);
  // A home saved before about: pages were refused still matches the chrome:// page it opens as.
  assert.equal(sameAddress('about:version', 'chrome://version/'), true);
  assert.equal(sameAddress('about:Version', 'chrome://settings/'), false);
  assert.equal(sameAddress('about:blank', 'chrome://blank/'), false);
  assert.equal(sameAddress('about:version#sol-check', 'chrome://version/#sol-check'), true);
  assert.equal(sameAddress('about:version?a=1', 'chrome://version/?a=1'), true);
  assert.equal(sameAddress('about:version#a', 'chrome://version/#b'), false);
  assert.equal(sameAddress('about:blank#a', 'chrome://blank/#a'), false);
});
