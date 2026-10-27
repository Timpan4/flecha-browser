// Tab sleep follows Dia's rules: freeze after 30 min of use elsewhere, discard after 4 h of it or
// a day, keep the 10 most recent from discard, freeze other Spaces' tabs, leave media alone.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { sleepState, protectedRecent, FREEZE_AFTER, DISCARD_AFTER, DISCARD_AFTER_WALL_CLOCK, STATE_DEBOUNCE } = await loadModule('chromium/shared/tabSleep.ts');

const now = 10 * DISCARD_AFTER_WALL_CLOCK;
const minute = 60_000;
const tab = (fields = {}) => ({ id: 't', lastActive: now, usedAt: 0, shown: false, inHiddenSpace: false, media: false, state: 'alive', ...fields });

test('a background tab is frozen after 30 minutes of use elsewhere and discarded after 4 hours', () => {
  assert.equal(sleepState(tab(), now, FREEZE_AFTER - minute, false), 'alive');
  assert.equal(sleepState(tab(), now, FREEZE_AFTER, false), 'frozen');
  assert.equal(sleepState(tab({ state: 'frozen' }), now, DISCARD_AFTER - minute, false), 'frozen');
  assert.equal(sleepState(tab({ state: 'frozen' }), now, DISCARD_AFTER, false), 'discarded');
});

test('a day on the wall clock discards whatever the use', () => {
  assert.equal(sleepState(tab({ lastActive: now - DISCARD_AFTER_WALL_CLOCK }), now, 0, false), 'discarded');
  assert.equal(sleepState(tab({ lastActive: now - DISCARD_AFTER_WALL_CLOCK + minute }), now, 0, false), 'alive');
});

test('Dia\'s tabSuspensionBlocklist pages (Notion) stay alive; other addresses don\'t', () => {
  for (const url of ['https://www.notion.so/a', 'https://dev.notion.so/', 'https://stg.notion.so/x', 'https://app.notion.com/p']) {
    assert.equal(sleepState(tab({ url }), now, DISCARD_AFTER, false), 'alive', url);
  }
  for (const url of ['https://notion.so/a', 'https://www.notion.so.example.com/', 'https://example.com/?u=https://www.notion.so/']) {
    assert.equal(sleepState(tab({ url }), now, DISCARD_AFTER, false), 'discarded', url);
  }
});

test('the 10 most recently used tabs are only frozen', () => {
  const tabs = Array.from({ length: 12 }, (_, i) => ({ id: `t${i}`, lastActive: now - i * minute }));
  const kept = protectedRecent(tabs);
  assert.deepEqual([...kept].sort(), tabs.slice(0, 10).map((t) => t.id).sort());
  assert.equal(sleepState(tab(), now, DISCARD_AFTER, true), 'frozen');
  assert.equal(sleepState(tab({ lastActive: now - DISCARD_AFTER_WALL_CLOCK }), now, 0, true), 'frozen');
});

test('a tab in a Space no window shows is frozen at once, and discarded by time as any other', () => {
  assert.equal(sleepState(tab({ inHiddenSpace: true }), now, 0, false), 'frozen');
  assert.equal(sleepState(tab({ inHiddenSpace: true }), now, DISCARD_AFTER, false), 'discarded');
  assert.equal(sleepState(tab({ inHiddenSpace: true }), now, DISCARD_AFTER, true), 'frozen');
});

test('a tab on screen or playing media stays alive', () => {
  assert.equal(sleepState(tab({ shown: true, state: 'frozen' }), now, DISCARD_AFTER, false), 'alive');
  assert.equal(sleepState(tab({ media: true }), now, DISCARD_AFTER, false), 'alive');
  assert.equal(sleepState(tab({ media: true, inHiddenSpace: true }), now, 0, false), 'alive');
});

test('a state that just changed stays for a minute; a discarded tab stays discarded', () => {
  assert.equal(sleepState(tab({ state: 'alive', changedAt: now - STATE_DEBOUNCE + 1 }), now, DISCARD_AFTER, false), 'alive');
  assert.equal(sleepState(tab({ state: 'alive', changedAt: now - STATE_DEBOUNCE }), now, DISCARD_AFTER, false), 'discarded');
  assert.equal(sleepState(tab({ state: 'discarded' }), now, 0, false), 'discarded');
});
