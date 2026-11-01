// A coding agent's tabs sit in a folder of their own above Today: Today's bulk closes leave them,
// and a tab you move is yours.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { Store } = await loadModule('chromium/shared/store.ts');

function withAgentTab() {
  const store = new Store({ load: () => undefined, save: () => {} });
  const windowId = store.focusedWindowId;
  // Newest first in Today: mine, then the agent's, then mine again.
  const older = store.addTab('https://example.com/older', { windowId });
  const agent = store.addTab('https://example.com/agent', { windowId, activate: false, agent: true });
  const newer = store.addTab('https://example.com/newer', { windowId });
  return { store, windowId, older: older.id, agent: agent.id, newer: newer.id };
}

test("Today's Clear, Close Tabs Below and Close Other Tabs leave the agent's folder", () => {
  for (const close of [
    (s) => s.store.clearToday(s.windowId),
    (s) => s.store.closeTodayTabs(s.newer, 'below'),
    (s) => s.store.closeTodayTabs(s.newer, 'others'),
  ]) {
    const s = withAgentTab();
    const closed = close(s);
    assert.ok(closed.includes(s.older));
    assert.ok(!closed.includes(s.agent));
    assert.ok(s.store.state.tabs[s.agent]);
  }
});

test("Close Other Tabs in the agent's folder leaves Today's tabs", () => {
  const s = withAgentTab();
  const second = s.store.addTab('https://example.com/agent-2', { windowId: s.windowId, activate: false, agent: true });
  assert.deepEqual(s.store.closeTodayTabs(second.id, 'others'), [s.agent]);
  assert.ok(s.store.state.tabs[s.older] && s.store.state.tabs[s.newer]);
});

test("moving an agent's tab to another Space makes it yours", () => {
  const s = withAgentTab();
  const work = s.store.newSpace(false).id;
  s.store.moveToSpace(s.agent, work);
  assert.equal(s.store.state.tabs[s.agent].agent, undefined);
});
