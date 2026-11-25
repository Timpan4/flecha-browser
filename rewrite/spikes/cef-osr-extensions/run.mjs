import assert from 'node:assert/strict';
import { spawn, execFileSync } from 'node:child_process';
import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { once } from 'node:events';
import { cp, mkdir, readFile, writeFile } from 'node:fs/promises';
import { createServer } from 'node:http';
import { createInterface } from 'node:readline';
import { setTimeout as delay } from 'node:timers/promises';

// Same per-step allowance and polling interval as the rewrite harness.
const stepTimeoutMs = 10_000;
const pollIntervalMs = 100;
const fixture = '/mnt/visual/extension';
const profileRoot = '/mnt/scratch';
const app = '/mnt/work/target/debug/flecha';

const seedCases = new Map([
  ['preseed', 'seed-chrome'], ['preseed-switch', 'switch'], ['preseed-policy', 'policy'],
]);

const defaultCases = [
  'none', 'legacy', 'switch', 'preseed-switch', 'port-plain', 'port', 'pipe', 'pipe-incognito',
  'seed-chrome', 'preseed', 'policy', 'preseed-policy',
];

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
  const signal = AbortSignal.timeout(stepTimeoutMs);
  const version = await fetch(`http://127.0.0.1:${port}/json/version`, { signal }).then(response => response.json());
  const socket = new WebSocket(version.webSocketDebuggerUrl);

  try {
    await once(socket, 'open', { signal });
  } catch (error) {
    socket.close();
    throw Error('Could not open the DevTools WebSocket', { cause: error });
  }

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
    'manifest.json', 'background.js', 'content.js', 'popup.html', 'popup.css', 'popup.js', 'icon.png', 'collector.json']);

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
let packed;
let holdUpdate = false;

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
  } else if (path.startsWith('/popup-report/')) {
    if (request.method !== 'POST' || !popupReports.has(path)) {
      response.writeHead(404).end();

      return;
    }

    let body = '';
    request.setEncoding('utf8').on('data', chunk => { body += chunk; });
    request.on('end', () => {
      response.setHeader('Access-Control-Allow-Origin', '*');

      try {
        const result = JSON.parse(body);
        assert(result && !Array.isArray(result) && (Object.hasOwn(result, 'id') || Object.hasOwn(result, 'error')));
        popupReports.set(path, result);
        response.end();
      } catch (cause) {
        popupReports.set(path, Error(`Invalid popup report for ${path}`, { cause }));
        response.writeHead(400).end();
      }
    });
  } else {
    response.setHeader('Content-Type', 'text/html; charset=utf-8');
    response.setHeader('Cache-Control', 'no-store');
    response.end('<!doctype html><title>Flecha fixture</title><p>Extension script fixture</p>');
  }
});

const args = process.argv.slice(2);
const cases = new Set();

for (const kind of args.length ? args : defaultCases) {
  assert(defaultCases.includes(kind), `Unknown extension probe case: ${kind}`);
  const seed = seedCases.get(kind);

  if (seed) cases.add(seed);
  cases.add(kind);
}

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

function expectPrefs(prefs, id, location, label) {
  if (location === undefined) {
    assert.deepEqual(prefs, [], `${label}: unexpected fixture preferences`);

    return;
  }

  assert.equal(prefs.length, 1, `${label}: expected one fixture preference record`);
  assert.equal(prefs[0].id, id, `${label}: unexpected fixture ID`);
  assert.equal(prefs[0].location, location, `${label}: unexpected install location`);
  assert.deepEqual(prefs[0].icons, { 16: 'icon.png' }, `${label}: missing fixture icon metadata`);
}

function expectReply(reply, pathname, incognito) {
  assert(reply && !Object.hasOwn(reply, 'error'), `${pathname}: worker reply failed: ${JSON.stringify(reply)}`);
  assert.equal(reply.echoed, pathname, `${pathname}: worker echoed the wrong message`);
  assert.equal(reply.stored, pathname, `${pathname}: worker read the wrong storage value`);
  assert(Number.isInteger(reply.tabId), `${pathname}: worker reply lacks a sender tab ID`);
  assert.equal(reply.incognito, incognito, `${pathname}: wrong sender incognito flag`);
}

async function run(kind) {
  const profile = `${profileRoot}/profile-${kind}`;
  const extraFlags = [];

  const env = {
    ...process.env,
    PROBE_PROFILE: profile,
    PROBE_URL: `${base}/${kind}/first`,
  };

  const source = seedCases.get(kind);

  if (source) {
    await cp(`${profileRoot}/profile-${source}`, profile, {
      recursive: true,
      filter: path => !path.endsWith('/DevToolsActivePort'),
    });
    const prefs = await installedPrefs(`${profile}/Default`);

    evidence(`${kind}/seed-prefs`, prefs);
    expectPrefs(prefs, source === 'policy' ? packed.id : unpackedId,
      { policy: 7, switch: 8, 'seed-chrome': 4 }[source], `${kind}/seed-prefs`);
  } else {
    await mkdir(profile);
  }

  if (kind === 'switch') extraFlags.push(`--load-extension=${fixture}`);

  if (kind === 'seed-chrome') env.PROBE_CHROME = '1';

  if (kind === 'port') extraFlags.push('--enable-unsafe-extension-debugging');

  if (kind.startsWith('pipe')) extraFlags.push('--remote-debugging-pipe');

  if (kind === 'port') env.PROBE_CEF_CDP = '1';
  const usesPolicy = kind === 'policy' || kind === 'preseed-policy';
  const checksContexts = ['switch', 'pipe', 'pipe-incognito', 'preseed-policy', 'policy'].includes(kind);

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

  const exit = new Promise(resolve => {
    child.once('close', (code, signal) => resolve({ code, signal }));
    child.once('error', error => resolve({ code: null, signal: null, error: error.message }));
  });

  const childOutput = [];
  const output = createInterface({ input: child.stdout });
  output.on('line', line => childOutput.push(line));
  child.stderr.setEncoding('utf8').on('data', chunk => traces.push(chunk));
  const pipe = kind.startsWith('pipe') ? pipeCdp(child) : null;
  let browser;
  let caseError;
  let exitError;
  let id;
  let installLocation;

  try {
    const ready = await waitFor('DevTools port', () => {
      assert(child.exitCode === null && child.signalCode === null, `${kind}: CEF exited before DevTools startup`);

      return readIfPresent(`${profile}/DevToolsActivePort`);
    });

    browser = await socketCdp(ready.split('\n')[0]);
    const call = browser.call;

    const pages = async (params) => {
      const result = await call('Target.getTargets', params);

      assert(Array.isArray(result?.targetInfos), `${kind}: could not list targets: ${JSON.stringify(result)}`);

      return result.targetInfos;
    };

    const targetFor = url => waitFor(url, async () => (await pages()).find(target => target.url === url));

    const attach = async url => {
      const { targetId } = await targetFor(url);
      const session = await call('Target.attachToTarget', { targetId, flatten: true });
      assert(session?.sessionId, `${kind}: could not attach to ${url}: ${JSON.stringify(session)}`);

      return session.sessionId;
    };

    const evaluate = async (sessionId, expression) => {
      const result = await call('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }, sessionId);
      assert(result?.result && !result.exceptionDetails, `${kind}: Runtime.evaluate failed: ${JSON.stringify(result)}`);

      return result.result.value;
    };

    const readContent = sessionId => evaluate(sessionId, '({...document.documentElement?.dataset})');

    const readyContent = async (sessionId, pathname) => {
      let data;

      try {
        // A single Runtime.evaluate is bounded by the CDP step timeout.
        data = await evaluate(sessionId, `new Promise(resolve => {
          const root = document.documentElement;
          const check = () => {
            if (root.hasAttribute('data-flecha-reply') || root.hasAttribute('data-flecha-error')) {
              observer.disconnect();
              resolve({ ...root.dataset });
            }
          };
          const observer = new MutationObserver(check);
          observer.observe(root, { attributes: true, attributeFilter: ['data-flecha-reply', 'data-flecha-error'] });
          check();
        })`);
      } catch (cause) {
        throw Error(`${pathname}: could not observe the content script reply`, { cause });
      }

      assert(!Object.hasOwn(data, 'flechaError'), `${pathname}: content script failed: ${data.flechaError}`);
      assert.equal(data.flechaInjected, 'loading', `${pathname}: missing document_start marker`);
      expectReply(JSON.parse(data.flechaReply), pathname, pathname.endsWith('/private'));

      return data;
    };

    const fixtureReady = (sessionId, url) => waitFor(url, () => evaluate(sessionId,
      `location.href === ${JSON.stringify(url)} &&
        document.querySelector('p')?.textContent === 'Extension script fixture' &&
        document.readyState === 'complete'`));

    const navigate = async (sessionId, url) => {
      const result = await call('Page.navigate', { url }, sessionId);

      assert(result?.frameId && !result.errorText, `${kind}: navigation to ${url} failed: ${JSON.stringify(result)}`);
      await fixtureReady(sessionId, url);
    };

    const noContent = async (sessionId, url) => {
      const data = await readContent(sessionId);

      assert.deepEqual(data, {}, `${url}: unexpected content script result`);

      return data;
    };

    const first = await attach(env.PROBE_URL);
    await fixtureReady(first, env.PROBE_URL);

    const firstContent = kind === 'switch' || kind === 'preseed-policy'
      ? await readyContent(first, `/${kind}/first`)
      : await noContent(first, env.PROBE_URL);

    const requestsAtFirstObservation = [...policyHits];

    evidence(`${kind}/first-document`, firstContent);

    if (usesPolicy) evidence(`${kind}/requests-at-first-observation`, requestsAtFirstObservation);

    const method = pipe ?? call;

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
      assert.equal(load?.message, 'Cannot install extension because it uses an unsupported manifest version.',
        `legacy: expected MV2 rejection, got ${JSON.stringify(load)}`);
    }

    if (pipe || kind.startsWith('port') || kind === 'seed-chrome') {
      const params = { path: fixture };

      if (pipe) params.enableInIncognito = kind === 'pipe-incognito';
      const loaded = await method('Extensions.loadUnpacked', params);

      evidence(`${kind}/load`, loaded);
      assert.equal(loaded?.id, unpackedId,
        `${kind}: Extensions.loadUnpacked failed for ${params.path}: ${JSON.stringify(loaded)}`);
      id = loaded.id;
      installLocation = 4;
    }

    if (kind === 'port') {
      const prefix = 'cef-cdp ';
      const line = await waitFor('CEF page CDP reply', () => childOutput.find(entry => entry.startsWith(prefix)));
      const result = JSON.parse(line.slice(prefix.length));

      evidence('port/cef-page-cdp', result);
      assert.deepEqual(result.error, { code: -32000, message: 'Method not available.' },
        'port: page CDP did not reject Extensions.loadUnpacked as expected');
    }

    if (usesPolicy) {
      id = packed.id;
      installLocation = 7;
      holdUpdate = false;

      for (const respond of updateResponses.splice(0)) respond();

      if (kind === 'policy') {
        await waitFor('policy CRX download', () => policyHits.includes('/probe.crx'));
        evidence('policy/requests', [...policyHits]);
      }

      await waitFor('policy worker', async () => (await pages()).some(target =>
        target.type === 'service_worker' && target.url === `chrome-extension://${id}/background.js`));
    }

    if (kind === 'switch') {
      id = unpackedId;
      installLocation = 8;
    }

    if (kind === 'preseed-switch') installLocation = 8;

    const list = await method('Extensions.getExtensions');
    const showsUnpacked = id && !usesPolicy;

    evidence(`${kind}/list`, list);
    assert.deepEqual(list?.extensions, showsUnpacked ? [{
      id, name: 'Flecha OSR extension probe', version: '1.0', path: fixture, enabled: true,
    }] : [], `${kind}: unexpected Extensions.getExtensions result`);

    const revisit = `${base}/${kind}/revisit`;
    await navigate(first, revisit);
    const stored = `/${kind}/revisit`;
    const content = id ? await readyContent(first, stored) : await noContent(first, revisit);
    const workerUrl = `chrome-extension://${id ?? unpackedId}/background.js`;

    const workers = (await pages()).filter(target => target.type === 'service_worker' && target.url === workerUrl)
      .map(target => target.url);

    evidence(`${kind}/revisit-content`, content);
    evidence(`${kind}/workers`, workers);
    assert.deepEqual(workers, id ? [workerUrl] : [], `${kind}: unexpected worker targets`);

    const openPopup = async (label, expectedStored) => {
      const path = `/popup-report/${label}`;
      const url = `chrome-extension://${id}/popup.html?report=${encodeURIComponent(path)}`;

      popupReports.set(path, null);

      try {
        child.stdin.write(`global ${url}\n`);

        const result = await waitFor(`${label} popup report`, () => {
          const report = popupReports.get(path);

          if (report instanceof Error) throw report;

          return report;
        });

        assert(!Object.hasOwn(result, 'error'), `${label}: popup APIs failed: ${result.error}`);
        assert.equal(result.id, id, `${label}: unexpected popup extension ID`);
        assert.equal(result.incognito, false, `${label}: expected a regular popup`);
        assert.equal(result.storage?.installed, true, `${label}: missing popup install marker`);
        assert.equal(result.storage?.content, expectedStored, `${label}: wrong popup storage value`);
        assert.equal(result.reply?.echoed, 'popup', `${label}: wrong popup worker reply: ${JSON.stringify(result.reply)}`);
        assert.equal(result.reply.stored, expectedStored, `${label}: wrong popup worker storage value`);
        assert(Number.isInteger(result.reply.tabId), `${label}: popup worker reply lacks a sender tab ID`);
        assert.equal(result.icon, true, `${label}: popup icon did not load`);

        if (kind === 'seed-chrome') {
          assert(result.active?.some(tab => tab.active && tab.url === url), `${label}: popup window did not see its active tab`);
          assert(result.all?.some(tab => tab.url === url), `${label}: tab inventory omitted the popup window`);
        } else {
          assert.deepEqual(result.active, [], `${label}: Alloy popup found an active tab`);
          assert.deepEqual(result.all, [], `${label}: Alloy popup found a tab list`);
        }

        return { result, url };
      } finally {
        popupReports.delete(path);
      }
    };

    if (id) {
      const { result, url } = await openPopup(kind, stored);

      evidence(`${kind}/popup`, result);

      if (kind !== 'seed-chrome') {
        const opened = await waitFor(`${kind} popup browser ID`, () => childOutput
          .find(line => line.startsWith('popup-browser ') && line.endsWith(` ${url}`)));

        const browserId = Number(opened.split(' ')[1]);

        assert(Number.isInteger(browserId) && browserId > 0, `${kind}: invalid popup browser ID`);

        const painted = await waitFor(`${kind} popup OnPaint on browser ${browserId}`, () => childOutput
          .find(line => line.startsWith(`popup-painted ${browserId} `)));

        evidence(`${kind}/popup-browser`, { browserId, url });
        evidence(`${kind}/paint`, painted);
      }
    }

    if (pipe || kind === 'seed-chrome') {
      const target = await targetFor(revisit);

      const tab = await waitFor(`${kind} tab target for ${revisit}`, async () =>
        (await pages({ filter: [{ type: 'tab' }] })).find(candidate => candidate.url === revisit));

      const pageAction = await method('Extensions.triggerAction', { id, targetId: target.targetId });
      const tabAction = await method('Extensions.triggerAction', { id, targetId: tab.targetId });

      evidence(`${kind}/trigger-page`, pageAction);
      evidence(`${kind}/tab-target`, tab.type);
      evidence(`${kind}/trigger-tab`, tabAction);
      assert.deepEqual(pageAction, { code: -32000, message: 'Action can only be triggered on a tab target.' },
        `${kind}: unexpected action result for a page target`);
      assert.deepEqual(tabAction, pipe ? { code: -32000, message: 'Tab target is not attached to a browser window.' } : {},
        `${kind}: unexpected action result for a tab target`);
    }

    if (checksContexts) {
      for (const context of ['global', 'space', 'private']) {
        const pathname = `/${kind}/${context}`;
        const url = base + pathname;
        const freshSpace = kind === 'policy' && context === 'space';

        if (freshSpace) holdUpdate = true;
        child.stdin.write(`${context} ${url}\n`);
        const session = await attach(url);
        await fixtureReady(session, url);

        const shouldInject = context === 'global' ||
          (context === 'space' && ['switch', 'preseed-policy'].includes(kind)) ||
          (context === 'private' && kind === 'pipe-incognito');

        const data = shouldInject ? await readyContent(session, pathname) : await noContent(session, url);

        evidence(`${kind}/context-${context}`, data);

        if (freshSpace) {
          holdUpdate = false;

          for (const respond of updateResponses.splice(0)) respond();
          await waitFor('Space policy installation', async () => (await pages()).filter(target =>
            target.type === 'service_worker' && target.url === `chrome-extension://${id}/background.js`).length > 1);
          await navigate(session, url + '/revisit');
          evidence('policy/context-space-after-install', await readyContent(session, pathname + '/revisit'));
        }
      }

      if (kind !== 'pipe') {
        const context = kind === 'pipe-incognito' ? 'private' : 'global';
        const { result } = await openPopup(`${kind}-after-contexts`, `/${kind}/${context}`);

        evidence(`${kind}/global-storage-after-contexts`, result.storage);
      }
    }
  } catch (error) {
    caseError = error;
    throw error;
  } finally {
    browser?.close();
    child.stdin.end('quit\n');
    let shutdownTimedOut = false;

    const shutdownTimer = setTimeout(() => {
      shutdownTimedOut = true;
      child.kill('SIGKILL');

      for (const stream of child.stdio) stream.destroy();
    }, stepTimeoutMs);

    try {
      const status = await exit;

      evidence(`${kind}/exit`, { ...status, shutdownTimedOut });

      if (shutdownTimedOut || status.code !== 0) {
        const reason = shutdownTimedOut
          ? `CEF shutdown timed out after ${stepTimeoutMs} ms; forced SIGKILL`
          : `CEF did not exit cleanly: ${JSON.stringify(status)}. See cef.log.`;

        exitError = Error(`${kind}: ${reason}`);

        if (caseError) console.error(exitError.message);
      }
    } finally {
      clearTimeout(shutdownTimer);
    }
  }

  if (exitError) throw exitError;

  const prefs = await installedPrefs(`${profile}/Default`);

  evidence(`${kind}/prefs`, prefs);
  expectPrefs(prefs, id ?? unpackedId, installLocation, `${kind}/prefs`);

  if (pipe) assert.equal(prefs[0].incognito, kind === 'pipe-incognito', `${kind}: wrong private opt-in preference`);

  if (checksContexts) {
    const spacePrefs = await installedPrefs(`${profile}/space`);

    evidence(`${kind}/space-prefs`, spacePrefs);
    expectPrefs(spacePrefs, id, pipe ? undefined : installLocation, `${kind}/space-prefs`);
  }
}

try {
  await writeFile(`${fixture}/collector.json`, JSON.stringify({ origin: base }));

  if (cases.has('policy')) packed = packExtension();

  for (const kind of cases) await run(kind);
} finally {
  server.close();
  server.closeAllConnections();
  await writeFile('/mnt/visual/results.json', JSON.stringify(items, null, 2) + '\n');
  await writeFile('/mnt/visual/cef.log', traces.join(''));
}
