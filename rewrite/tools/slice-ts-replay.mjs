import { fileURLToPath } from 'node:url';
import { loadModule } from '../../chromium/tests/load-module.mjs';

const { Store } = await loadModule(fileURLToPath(new URL('../../chromium/shared/store.ts', import.meta.url)));
const store = new Store({ load: () => undefined, save: () => {} });
const window = store.state.windows[0];
const activeSpace = () => window.spaces[window.activeSpaceId];
const names = new Map();
const trace = [];

function record(step) {
  const space = activeSpace();
  const name = id => names.get(id);
  trace.push({
    step, today: space.today.map(name), active: name(space.activeTabId),
    split: space.split?.map(name) ?? null,
  });
}

const left = store.addTab('http://x/open', { activate: true, windowId: window.id }).id;

names.set(left, 'L');

record('open');

const right = store.addTab('http://x/split', {
  activate: false, afterTabId: activeSpace().activeTabId, windowId: window.id,
}).id;

names.set(right, 'R');

store.splitWith(right, 'right', window.id);

record('split');

const peek = store.addTab('http://x/peek', {
  activate: true, afterTabId: activeSpace().activeTabId, windowId: window.id,
}).id;

names.set(peek, 'P');

record('promote');

store.closeTab(peek);

record('detach');

const moved = store.addTab('http://x/peek', { activate: true, windowId: window.id }).id;

names.set(moved, 'M');

record('return');

store.closeTab(activeSpace().activeTabId);

record('close');

for (const entry of trace) console.log(JSON.stringify(entry));
