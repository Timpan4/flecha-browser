// The downloads list names each file, whatever the platform's path separator.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

globalThis.chrome = { storage: { session: { get: async () => ({}), set: async () => {} } } };
const { toInfo } = await loadModule('chromium/shell/downloads.ts');

const item = (filename) => ({ id: 1, filename, url: 'https://example.com/get', finalUrl: '', bytesReceived: 0, totalBytes: 10, state: 'complete', startTime: '2026-10-27T10:00:00Z', canResume: false });

test('a download is listed by its file name on Linux, macOS and Windows', () => {
  assert.equal(toInfo(item('/home/me/Downloads/report.pdf')).filename, 'report.pdf');
  assert.equal(toInfo(item('C:\\Users\\me\\Downloads\\report.pdf')).filename, 'report.pdf');
  assert.equal(toInfo(item('\\\\server\\share\\report.pdf')).filename, 'report.pdf');
  assert.equal(toInfo(item('C:/Users/me\\Downloads\\report.pdf')).filename, 'report.pdf');
  assert.equal(toInfo(item('C:\\Users\\me\\Downloads\\report.pdf')).path, 'C:\\Users\\me\\Downloads\\report.pdf');
  // Elsewhere a backslash can be part of the name.
  assert.equal(toInfo(item('/home/me/Downloads/a\\b.txt')).filename, 'a\\b.txt');
});
