// The site panel's shield: ad and tracker blocking turned off per site, with its subdomains.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { loadModule } from './load-module.mjs';

const { blocksAdsOn, isSiteHost } = await loadModule('chromium/shared/adBlockSites.ts');
const { Store } = await loadModule('chromium/shared/store.ts');
const { decodeState } = await loadModule('chromium/shared/savedState.ts');

const newStore = () => new Store({ load: () => undefined, save: () => {} });

test('turning blocking off for a site covers its subdomains and leaves other sites blocked', () => {
  const store = newStore();
  store.setAdBlockOn('example.com', false);
  assert.equal(blocksAdsOn(store.state.adBlockAllowed, 'example.com'), false);
  assert.equal(blocksAdsOn(store.state.adBlockAllowed, 'news.example.com'), false);
  assert.equal(blocksAdsOn(store.state.adBlockAllowed, 'notexample.com'), true);
  assert.equal(blocksAdsOn(store.state.adBlockAllowed, 'example.org'), true);
});

test('turning it off twice keeps one entry, and a covered subdomain adds none', () => {
  const store = newStore();
  store.setAdBlockOn('example.com', false);
  store.setAdBlockOn('example.com', false);
  store.setAdBlockOn('www.example.com', false);
  assert.deepEqual(store.state.adBlockAllowed, ['example.com']);
});

test('turning blocking back on from a subdomain removes the entry that covered it', () => {
  const store = newStore();
  store.setAdBlockOn('example.com', false);
  store.setAdBlockOn('other.org', false);
  store.setAdBlockOn('www.example.com', true);
  assert.equal(blocksAdsOn(store.state.adBlockAllowed, 'www.example.com'), true);
  assert.deepEqual(store.state.adBlockAllowed, ['other.org']);
});

test('an emptied list stays saved as empty, so incognito windows follow it', () => {
  const store = newStore();
  store.setAdBlockOn('example.com', false);
  store.setAdBlockOn('example.com', true);
  assert.deepEqual(store.state.adBlockAllowed, []);
});

test('only hostnames Chromium takes as request domains count as sites', () => {
  assert.equal(isSiteHost('example.com'), true);
  assert.equal(isSiteHost('localhost'), true);
  assert.equal(isSiteHost('127.0.0.1'), true);
  assert.equal(isSiteHost('xn--bcher-kva.example'), true);
  assert.equal(isSiteHost('[::1]'), false);
  assert.equal(isSiteHost('Example.com'), false);
  assert.equal(isSiteHost(''), false);
});

test('the list survives a restart, and one with a hostname Chromium would refuse is dropped', () => {
  let json;
  const store = new Store({ load: () => undefined, save: (j) => (json = j) });
  store.setAdBlockOn('example.com', false);
  store.saveNow();
  assert.deepEqual(decodeState(json).adBlockAllowed, ['example.com']);

  store.state.adBlockAllowed.push('Bad Host');
  store.saveNow();
  assert.equal(decodeState(json).adBlockAllowed, undefined);
});
