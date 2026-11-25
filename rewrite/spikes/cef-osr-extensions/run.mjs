import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { once } from 'node:events';
import { cp, mkdir, readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { setTimeout as delay } from 'node:timers/promises';

// Same per-step allowance and polling interval as the rewrite harness.
const stepTimeoutMs = 10_000;
const pollIntervalMs = 100;
const fixture = '/mnt/visual/extension';
const profileRoot = '/mnt/scratch';
const app = '/mnt/work/target/debug/flecha';
const items = [];
const traces = [];
const extensionId = hash => [...hash.toString('hex')]
  .map(hex => 'abcdefghijklmnop'[parseInt(hex, 16)]).join('');
const unpackedId = extensionId(createHash('sha256').update(fixture).digest().subarray(0, 16));

const evidence = (check, result) => {
  const entry = { check, result };
  items.push(entry);
  console.log(JSON.stringify(entry));
};

async function waitFor(description, check) {
  const signal = AbortSignal.timeout(stepTimeoutMs);
  try {
    while (true) {
      signal.throwIfAborted();
      const result = await check();
      if (result) return result;
      await delay(pollIntervalMs, undefined, { signal });
    }
  } catch (error) {
    if (signal.aborted) throw Error(`Timed out: ${description}`, { cause: error });
    throw error;
  }
}

function cdp(receive, send) {
  let id = 0;
  const pending = new Map();
  receive(message => {
    const response = JSON.parse(message);
    const callback = pending.get(response.id);
    if (callback) callback(response.error ?? response.result);
  });
  return (method, params = {}, sessionId) => new Promise((resolve, reject) => {
    const requestId = ++id;
    const timer = setTimeout(() => {
      pending.delete(requestId);
      reject(Error(`CDP did not answer ${method}`));
    }, stepTimeoutMs);
    pending.set(requestId, result => {
      clearTimeout(timer);
      pending.delete(requestId);
      resolve(result);
    });
    send(JSON.stringify({ id: requestId, method, params, sessionId }));
  });
}

function pipeCdp(child) {
  return cdp(process => {
    let received = '';
    child.stdio[4].setEncoding('utf8').on('data', chunk => {
      received += chunk;
      const messages = received.split('\0');
      received = messages.pop();
      for (const message of messages) process(message);
    });
  }, message => child.stdio[3].write(message + '\0'));
}

async function socketCdp(port) {
  const version = await fetch(`http://127.0.0.1:${port}/json/version`).then(response => response.json());
  const socket = new WebSocket(version.webSocketDebuggerUrl);
  await once(socket, 'open', { signal: AbortSignal.timeout(stepTimeoutMs) });
  return {
    call: cdp(process => socket.addEventListener('message', event => process(event.data)),
      message => socket.send(message)),
    close: () => socket.close(),
  };
}

function protobufBytes(tag, bytes) {
  const varint = value => {
    const encoded = [];
    while (value > 127) {
      encoded.push((value & 127) | 128);
      value >>>= 7;
    }
    return Buffer.from([...encoded, value]);
  };
  return Buffer.concat([varint((tag << 3) | 2), varint(bytes.length), bytes]);
}

function littleEndian(value) {
  const buffer = Buffer.alloc(4);
  buffer.writeUInt32LE(value);
  return buffer;
}

// CRX3 format in components/crx_file/crx3.proto. The generated signing key
// exists only for this local fixture and never leaves the runner process.
function packExtension() {
  const { publicKey, privateKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
  const der = publicKey.export({ type: 'spki', format: 'der' });
  const digest = createHash('sha256').update(der).digest().subarray(0, 16);
  const id = extensionId(digest);
  const zip = execFileSync('bsdtar', ['--format', 'zip', '-cf', '-', '-C', fixture,
    'manifest.json', 'background.js', 'content.js', 'popup.html', 'popup.css', 'popup.js', 'icon.png']);
  const signedData = protobufBytes(1, digest);
  const toSign = Buffer.concat([Buffer.from('CRX3 SignedData\0'),
    littleEndian(signedData.length), signedData, zip]);
  const proof = Buffer.concat([protobufBytes(1, der), protobufBytes(2, sign('sha256', toSign, privateKey))]);
  const header = Buffer.concat([protobufBytes(2, proof), protobufBytes(10000, signedData)]);
  const crx = Buffer.concat([Buffer.from('Cr24'), littleEndian(3), littleEndian(header.length), header, zip]);
  return { id, crx };
}

const policyHits = [];
const updateResponses = [];
const popupReports = new Map();
const server = createServer((request, response) => {
  const path = new URL(request.url, base).pathname;
  if (path === '/update.xml') {
    policyHits.push(path);
    response.setHeader('Content-Type', 'application/xml');
    const respond = () => response.end(`<?xml version="1.0" encoding="UTF-8"?>
      <gupdate xmlns="http://www.google.com/update2/response" protocol="2.0">
        <app appid="${packed.id}"><updatecheck codebase="${base}/probe.crx" version="1.0"/></app>
      </gupdate>`);
    if (holdUpdate) updateResponses.push(respond);
    else respond();
  } else if (path === '/probe.crx') {
    policyHits.push(path);
    response.setHeader('Content-Type', 'application/x-chrome-extension');
    response.end(packed.crx);
  } else if (path.startsWith('/popup-report/') && request.method === 'POST') {
    let body = '';
    request.setEncoding('utf8').on('data', chunk => { body += chunk; });
    request.on('end', () => {
      popupReports.set(path, JSON.parse(body));
      response.setHeader('Access-Control-Allow-Origin', '*');
      response.end();
    });
  } else {
    response.setHeader('Content-Type', 'text/html; charset=utf-8');
    response.setHeader('Cache-Control', 'no-store');
    response.end('<!doctype html><title>Flecha fixture</title><p>Extension script fixture</p>');
  }
});

const args = process.argv.slice(2);
const cases = args.length ? args : [
  'none', 'legacy', 'switch', 'preseed-switch', 'port-plain', 'port', 'pipe', 'pipe-incognito',
  'seed-chrome', 'preseed', 'policy', 'preseed-policy',
];
const packed = cases.includes('policy') ? packExtension() : undefined;
let holdUpdate = false;
server.listen(0, '127.0.0.1');
await once(server, 'listening');
const base = `http://127.0.0.1:${server.address().port}`;
const flags = ['--remote-debugging-port=0', '--disable-gpu', '--enable-logging=stderr'];

async function readIfPresent(path) {
  try {
    return await readFile(path, 'utf8');
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
}

async function installedPrefs(profile) {
  const found = [];
  for (const name of ['Preferences', 'Secure Preferences']) {
    const file = `${profile}/${name}`;
    const raw = await readIfPresent(file);
    const extensions = raw && JSON.parse(raw).extensions?.settings;
    for (const [id, entry] of Object.entries(extensions ?? {})) {
      if (entry.path === fixture || entry.manifest?.name === 'Flecha OSR extension probe') {
        const directory = entry.path.startsWith('/') ? entry.path : `${profile}/Extensions/${entry.path}`;
        const manifest = entry.manifest ?? JSON.parse(await readFile(`${directory}/manifest.json`, 'utf8'));
        found.push({ name, id, path: entry.path, location: entry.location,
          icons: manifest.icons ?? null, incognito: entry.incognito ?? null });
      }
    }
  }
  return found;
}

async function run(kind) {
  assert(['none', 'legacy', 'switch', 'preseed-switch', 'port-plain', 'port', 'pipe', 'pipe-incognito',
    'seed-chrome', 'preseed', 'policy', 'preseed-policy'].includes(kind));
  const profile = `${profileRoot}/profile-${kind}`;
  const extraFlags = [];
  const env = {
    ...process.env,
    PROBE_PROFILE: profile,
    PROBE_URL: `${base}/${kind}/first`,
  };
  if (kind.startsWith('preseed')) {
    const source = { preseed: 'seed-chrome', 'preseed-switch': 'switch', 'preseed-policy': 'policy' }[kind];
    await cp(`${profileRoot}/profile-${source}`, profile, {
      recursive: true,
      filter: path => !path.endsWith('/DevToolsActivePort'),
    });
    evidence(`${kind}/seed-prefs`, await installedPrefs(`${profile}/Default`));
  } else {
    await mkdir(profile);
  }
  if (kind === 'switch') extraFlags.push(`--load-extension=${fixture}`);
  if (kind === 'seed-chrome') env.PROBE_CHROME = '1';
  if (kind === 'port') extraFlags.push('--enable-unsafe-extension-debugging');
  if (kind.startsWith('pipe')) extraFlags.push('--remote-debugging-pipe');
  if (kind === 'port') env.PROBE_CEF_CDP = '1';
  const usesPolicy = kind === 'policy' || kind === 'preseed-policy';
  if (usesPolicy) {
    holdUpdate = true;
    policyHits.length = 0;
    env.PROBE_POLICY = `${profileRoot}/probe-policy`;
    await mkdir(`${env.PROBE_POLICY}/managed`, { recursive: true });
    await writeFile(`${env.PROBE_POLICY}/managed/extensions.json`,
      JSON.stringify({ ExtensionInstallForcelist: [`${packed.id};${base}/update.xml`] }));
  }

  const child = spawn(app, [...flags, ...extraFlags], {
    env,
    stdio: ['pipe', 'pipe', 'pipe', 'pipe', 'pipe'],
  });
  child.stdin.on('error', () => {});
  child.stdio[3].on('error', () => {});
  const exit = once(child, 'close');
  const childOutput = [];
  child.stdout.setEncoding('utf8').on('data', chunk => childOutput.push(chunk));
  child.stderr.setEncoding('utf8').on('data', chunk => traces.push(chunk));
  const pipe = kind.startsWith('pipe') ? pipeCdp(child) : null;
  let browser;
  try {
    const ready = await waitFor('DevTools port', () => {
      assert.equal(child.exitCode, null, 'CEF exited before DevTools startup');
      return readIfPresent(`${profile}/DevToolsActivePort`);
    });
    browser = await socketCdp(ready.split('\n')[0]);
    const call = browser.call;
    const pages = async () => (await call('Target.getTargets')).targetInfos;
    const targetFor = url => waitFor(url, async () => (await pages()).find(target => target.url === url));
    const attach = async url => {
      const { targetId } = await targetFor(url);
      const session = await call('Target.attachToTarget', { targetId, flatten: true });
      assert(session.sessionId, JSON.stringify(session));
      return session.sessionId;
    };
    const evaluate = async (sessionId, expression) => {
      const result = await call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }, sessionId);
      assert(!result.code && !result.exceptionDetails, JSON.stringify(result));
      return result.result.value;
    };
    const readContent = sessionId => evaluate(sessionId, '({...document.documentElement?.dataset})');
    const readyContent = sessionId => waitFor('content script reply', async () => {
      const data = await readContent(sessionId);
      return data.flechaReply || data.flechaError ? data : false;
    });
    const fixtureReady = (sessionId, url) => waitFor(url, () => evaluate(sessionId,
      `location.href === ${JSON.stringify(url)} &&
        document.querySelector('p')?.textContent === 'Extension script fixture' &&
        document.readyState === 'complete'`));
    const first = await attach(env.PROBE_URL);
    await fixtureReady(first, env.PROBE_URL);
    evidence(`${kind}/first-document`, await readContent(first));

    const method = pipe ?? call;
    let id;
    if (kind === 'legacy') {
      const directory = '/mnt/scratch/legacy-extension';
      await mkdir(directory);
      await writeFile(`${directory}/manifest.json`, JSON.stringify({
        manifest_version: 2,
        name: 'Flecha MV2 background page probe',
        version: '1.0',
        background: { scripts: ['background.js'], persistent: true },
      }));
      await writeFile(`${directory}/background.js`, 'chrome.runtime.onInstalled.addListener(() => {});');
      const load = await call('Extensions.loadUnpacked', { path: directory });
      evidence('legacy/load', load);
      assert(load.code && !load.id);
    }
    if (kind.startsWith('port') || kind === 'seed-chrome') {
      const loaded = await call('Extensions.loadUnpacked', { path: fixture });
      evidence(`${kind}/load`, loaded);
      id = loaded.id;
      if (kind === 'port') evidence('port/cef-page-cdp', JSON.parse(
        (await waitFor('CEF CDP reply', () =>
          childOutput.join('').split('\n').find(line => line.startsWith('cef-cdp ')))).slice(8)));
    }
    if (pipe) {
      const loaded = await pipe('Extensions.loadUnpacked', {
        path: fixture,
        enableInIncognito: kind === 'pipe-incognito',
      });
      evidence(`${kind}/load`, loaded);
      id = loaded.id;
    }

    if (usesPolicy) {
      id = packed.id;
      evidence(`${kind}/requests-at-first-document`, [...policyHits]);
      holdUpdate = false;
      for (const respond of updateResponses.splice(0)) respond();
      if (kind === 'policy') {
        await waitFor('policy CRX download', () => policyHits.includes('/probe.crx'));
        evidence('policy/requests', [...policyHits]);
      }
      await waitFor('policy worker', async () => (await pages()).some(target =>
        target.type === 'service_worker' && target.url === `chrome-extension://${id}/background.js`));
    }

    if (kind === 'preseed' || kind === 'switch') {
      id = unpackedId;
    }
    evidence(`${kind}/list`, await method('Extensions.getExtensions'));
    const revisit = `${base}/${kind}/revisit`;
    await call('Page.navigate', { url: revisit }, first);
    await fixtureReady(first, revisit);
    const content = id && kind !== 'preseed'
      ? await readyContent(first)
      : await readContent(first);
    if (id && kind !== 'preseed') assert.equal(JSON.parse(content.flechaReply).stored, `/${kind}/revisit`);
    evidence(`${kind}/revisit-content`, content);
    evidence(`${kind}/workers`, (await pages()).filter(target =>
      target.type === 'service_worker' && target.url === `chrome-extension://${id ?? unpackedId}/background.js`)
      .map(target => target.url));

    const openPopup = async (context, label) => {
      const path = `/popup-report/${label}`;
      const pop = `chrome-extension://${id}/popup.html?report=${encodeURIComponent(base + path)}`;
      child.stdin.write(`${context} ${pop}\n`);
      return waitFor('popup APIs', () => popupReports.get(path));
    };
    if (id && content.flechaReply) {
      const popupResult = await openPopup('global', kind);
      assert.equal(popupResult.id, id);
      assert.equal(popupResult.reply.echoed, 'popup');
      assert.equal(popupResult.icon, true);
      evidence(`${kind}/popup`, popupResult);
      if (kind !== 'seed-chrome') evidence(`${kind}/paint`,
        await waitFor('popup OnPaint pixel', () => childOutput.join('').split('\n')
          .find(line => line.startsWith('popup-painted '))));
    }

    if ((pipe || kind === 'seed-chrome') && id) {
      const target = await targetFor(revisit);
      const tabTargets = (await call('Target.getTargets', { filter: [{ type: 'tab' }] })).targetInfos;
      evidence(`${kind}/trigger-page`, await method('Extensions.triggerAction',
        { id, targetId: target.targetId }));
      const tab = tabTargets.find(candidate => candidate.url === revisit);
      evidence(`${kind}/tab-target`, tab?.type ?? null);
      if (tab) evidence(`${kind}/trigger-tab`, await method('Extensions.triggerAction', { id, targetId: tab.targetId }));
    }

    if (['switch', 'pipe', 'pipe-incognito', 'preseed-policy', 'policy'].includes(kind)) {
      for (const context of ['global', 'space', 'private']) {
        const url = `${base}/${kind}/${context}`;
        child.stdin.write(`${context} ${url}\n`);
        const session = await attach(url);
        await fixtureReady(session, url);
        const shouldInject = context === 'global' ||
          (context === 'space' && ['switch', 'preseed-policy'].includes(kind)) ||
          (context === 'private' && kind === 'pipe-incognito');
        const data = shouldInject ? await readyContent(session) : await readContent(session);
        evidence(`${kind}/context-${context}`, data);
        if (shouldInject) assert.equal(JSON.parse(data.flechaReply).stored, `/${kind}/${context}`);
        if (kind === 'policy' && context === 'space') {
          await waitFor('Space policy installation', async () => (await pages()).filter(target =>
            target.type === 'service_worker' && target.url === `chrome-extension://${id}/background.js`).length > 1);
          const afterInstall = url + '/revisit';
          await call('Page.navigate', { url: afterInstall }, session);
          await fixtureReady(session, afterInstall);
          evidence('policy/context-space-after-install', await readyContent(session));
        }
      }
      if (['switch', 'policy', 'preseed-policy', 'pipe-incognito'].includes(kind)) {
        const result = await openPopup('global', `${kind}-after-contexts`);
        evidence(`${kind}/global-storage-after-contexts`, result.storage);
      }
    }
  } finally {
    browser?.close();
    child.stdin.end('quit\n');
    const [code, signal] = await exit;
    evidence(`${kind}/exit`, { code, signal });
    assert.equal(code, 0, traces.join('').slice(-4000));
  }
  evidence(`${kind}/prefs`, await installedPrefs(`${profile}/Default`));
  if (['switch', 'pipe', 'pipe-incognito', 'preseed-policy', 'policy'].includes(kind)) {
    evidence(`${kind}/space-prefs`, await installedPrefs(`${profile}/space`));
  }
}

try {
  for (const kind of cases) await run(kind);
} finally {
  server.close();
  await writeFile('/mnt/visual/results.json', JSON.stringify(items, null, 2) + '\n');
  await writeFile('/mnt/visual/cef.log', traces.join(''));
}
