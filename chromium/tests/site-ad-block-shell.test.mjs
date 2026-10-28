// The site panel's shield in the background: the page it reloads.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const reloads = [];
let release;
const updating = new Promise((resolve) => (release = resolve));
globalThis.chrome = {
  extension: { inIncognitoContext: false },
  runtime: { getURL: (path) => `chrome-extension://flecha/${path}` },
  declarativeNetRequest: {
    RuleActionType: { ALLOW_ALL_REQUESTS: 'allowAllRequests' },
    ResourceType: { MAIN_FRAME: 'main_frame' },
    updateEnabledRulesets: async () => {},
    // Chromium takes its time; the user can switch tabs meanwhile.
    updateDynamicRules: () => updating,
  },
  scripting: { getRegisteredContentScripts: async () => [], registerContentScripts: async () => {} },
  tabs: { reload: async (id) => void reloads.push(id) },
};
globalThis.fetch = async () => ({ ok: true, json: async () => [] });

const { registerSiteControls, call, Store } = await loadModule('chromium/tests/site-ad-block-entry.ts');

test('the page reloaded is the one the shield was changed on, even if another is on screen by then', async () => {
  const store = new Store({ load: () => undefined, save: () => {} });
  let active = 101;
  registerSiteControls({ store, activeChromeId: () => active, mainWindow: async () => 1 });
  const done = call('setSiteAdBlock', 'example.com', false);
  active = 202;
  release();
  await done;
  assert.deepEqual(reloads, [101]);
  assert.deepEqual(store.state.adBlockAllowed, ['example.com']);
});
