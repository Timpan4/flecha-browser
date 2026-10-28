// A bookmarks file exported from any browser (Netscape HTML) becomes one tidied Space of bookmarks.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import fs from 'node:fs/promises';
import { loadModule } from './load-module.mjs';

const { readBookmarksHtml } = await loadModule('chromium/native-host/browserImport.ts');
const sample = await fs.readFile('chromium/native-host/fixtures/bookmarks.html', 'utf8');

test('an exported bookmarks file keeps its folders, with the toolbar as the top level', () => {
  const data = readBookmarksHtml(sample, 'bookmarks.html');
  assert.deepEqual(data.bookmarks, [
    { kind: 'tab', url: 'https://example.com/', title: 'Example' },
    {
      kind: 'folder',
      name: 'Work',
      children: [
        { kind: 'tab', url: 'https://github.com/lucianodiisouza/flecha', title: 'Flecha & friends' },
        {
          kind: 'folder',
          name: 'Docs',
          children: [
            { kind: 'tab', url: 'https://developer.chrome.com/docs/extensions', title: 'Chrome extensions – docs' },
            { kind: 'tab', url: 'https://nodejs.org/api/', title: 'https://nodejs.org/api/' },
          ],
        },
      ],
    },
    { kind: 'folder', name: 'Other bookmarks', children: [{ kind: 'tab', url: 'https://news.ycombinator.com/', title: 'Hacker News' }] },
  ]);
  // The empty folder, the bookmarklet and the second copy of example.com are left out, as from a browser.
  assert.equal(data.bookmarkCount, 5);
  assert.deepEqual(data.history, []);
  assert.deepEqual(data.warnings, []);
});

test('a file without bookmarks says which file and what to choose instead', () => {
  const data = readBookmarksHtml('<html><body><p>Notes</p></body></html>', 'notes.html');
  assert.equal(data.bookmarks, undefined);
  assert.equal(data.bookmarkCount, 0);
  assert.deepEqual(data.warnings, ['No bookmarks were found in notes.html. Choose the HTML file a browser saves when it exports its bookmarks.']);
});
