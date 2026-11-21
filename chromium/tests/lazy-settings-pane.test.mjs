import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { lazySettingsPane } = await loadModule('chromium/shell/ui/lazySettingsPane.ts');

test('a failed Settings pane explains the error and can initialize on a later visit', async t => {
  const classes = new Set();
  const alerts = new Set();
  const pane = {
    classList: { add: name => classes.add(name), remove: name => classes.delete(name) },
    prepend: node => alerts.add(node),
  };
  globalThis.document = { createElement: () => ({
    setAttribute(name, value) { this[name] = value; },
    remove() { alerts.delete(this); },
  }) };
  t.mock.method(console, 'error', () => {});
  const firstImport = Promise.withResolvers();
  let attempts = 0;
  const visit = lazySettingsPane(async () => {
    attempts++;
    if (attempts === 1) await firstImport.promise;
    if (attempts === 2) throw new Error('Setup failed');
  });
  const first = visit(pane, 'Tabs & Spaces');
  assert.equal(visit(pane, 'Tabs & Spaces'), first);
  firstImport.reject(new Error('Import failed'));
  await first;
  assert.ok(classes.has('pane-load-failed'));
  assert.deepEqual([...alerts].map(({ role, textContent }) => [role, textContent]), [
    ['alert', 'Could not load Tabs & Spaces. Open another pane, then return to try again.'],
  ]);

  await visit(pane, 'Tabs & Spaces');
  assert.equal(alerts.size, 1);
  await visit(pane, 'Tabs & Spaces');
  assert.equal(alerts.size, 0);
  assert.equal(classes.size, 0);
  await visit(pane, 'Tabs & Spaces');
  assert.equal(attempts, 3);
});
