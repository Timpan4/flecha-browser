// Drives the Rust agent server with the MCP TypeScript SDK's own client, the one today's coding
// agents use against the TS bridge.
//
//   node rewrite/crates/flecha-services/scripts/e2e-client.mjs <path to the fake_agent_server example>
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { once } from 'node:events';
import { mkdtemp, readFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createInterface } from 'node:readline';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { StreamableHTTPClientTransport } from '@modelcontextprotocol/sdk/client/streamableHttp.js';

const binary = process.argv[2];

if (!binary) throw new Error('usage: e2e-client.mjs <fake_agent_server binary>');

// The SDK's export map serves a stub package.json, so read the installed one.
const sdkVersion = JSON.parse(await readFile(new URL('../../../../node_modules/@modelcontextprotocol/sdk/package.json', import.meta.url), 'utf8')).version;
const fixture = JSON.parse(await readFile(new URL('../tests/fixtures/agent-tools.json', import.meta.url), 'utf8'));

const dir = await mkdtemp(join(tmpdir(), 'flecha-mcp-e2e-'));
const server = spawn(binary, [join(dir, 'agent-token')], { stdio: ['pipe', 'pipe', 'inherit'] });

try {
  const [endpoint] = await once(createInterface({ input: server.stdout }), 'line');
  const url = new URL(endpoint);
  assert.equal(url.hostname, '127.0.0.1');
  // Flecha's own bridge may be listening on 7390-7399; the fake must stay off them.
  assert.ok(Number(url.port) < 7390 || Number(url.port) > 7399, url.port);
  const token = await readFile(join(dir, 'agent-token'), 'utf8');

  const connect = async (bearer) => {
    const client = new Client({ name: 'flecha-e2e', version: '1' });
    const transport = new StreamableHTTPClientTransport(url, { requestInit: { headers: { Authorization: `Bearer ${bearer}` } } });
    await client.connect(transport);

    return { client, transport };
  };

  const { client, transport } = await connect(token);
  const checks = [];
  const check = (name, fn) => checks.push([name, fn]);

  check('initialize negotiates the TS server revision', () => {
    assert.equal(transport.protocolVersion, fixture.protocolVersions.latest);
    assert.deepEqual(client.getServerVersion(), fixture.serverInfo);
    assert.deepEqual(client.getServerCapabilities(), fixture.capabilities);
  });
  check('tools/list matches agentTools.ts', async () => {
    const { tools } = await client.listTools();
    assert.deepEqual(
      tools.map(({ name, description, inputSchema }) => ({ name, description, inputSchema })),
      fixture.tools.map(({ name, description, inputSchema }) => ({ name, description, inputSchema })),
    );
  });
  const call = async (name, args) => client.callTool({ name, arguments: args });
  const textOf = (result) => result.content[0].text;
  let agentTab;
  check('open_tab places a background agent tab', async () => {
    const result = await call('open_tab', { url: 'example.org' });
    assert.equal(result.isError, undefined);
    const opened = JSON.parse(textOf(result));
    assert.equal(opened.url, 'https://example.org');
    agentTab = opened.tabId;
    const tabs = JSON.parse(textOf(await call('list_tabs', {})));
    assert.deepEqual(
      tabs.map(({ id, active, agent }) => ({ id, active, agent })),
      [
        { id: agentTab, active: false, agent: true },
        { id: 'user-tab', active: true, agent: false },
      ],
    );
  });
  check('read_page, evaluate, click and wait_for reach the page', async () => {
    assert.equal(textOf(await call('read_page', { tabId: 'user-tab', maxChars: 7 })), '# Example\nhttps://example.com/\n\nExample');
    assert.equal(textOf(await call('evaluate', { tabId: 'user-tab', expression: 'document.title' })), 'Example');
    assert.equal(textOf(await call('evaluate', { tabId: 'user-tab', expression: '[1, 2]' })), '[\n  1,\n  2\n]');
    assert.equal(JSON.parse(textOf(await call('click', { tabId: 'user-tab', selector: 'h1' }))).clicked, 'h1');
    assert.equal(textOf(await call('wait_for', { tabId: 'user-tab', selector: 'h1' })), 'ready');
    const late = await call('wait_for', { tabId: 'user-tab', selector: '#never', timeoutMs: 300 });
    assert.deepEqual([late.isError, textOf(late)], [true, 'Timed out after 300ms']);
    const shot = await call('screenshot', { tabId: agentTab });
    assert.equal(shot.content[0].type, 'image');
  });
  check('bad arguments and unknown tabs come back as tool errors', async () => {
    const invalid = await call('read_page', { tabId: 'user-tab', format: 'pdf' });
    assert.equal(invalid.isError, true);
    assert.equal(
      textOf(invalid),
      'MCP error -32602: Input validation error: Invalid arguments for tool read_page: Invalid option: expected one of "text"|"html"|"links" at format',
    );
    assert.equal(textOf(await call('evaluate', { tabId: 'nope', expression: '1' })), 'No tab with id nope');
  });
  check('close_tab removes the agent tab', async () => {
    assert.equal(textOf(await call('close_tab', { tabId: agentTab })), 'ok');
    assert.equal(JSON.parse(textOf(await call('list_tabs', {}))).length, 1);
  });
  check('a wrong token cannot connect', async () => {
    await assert.rejects(connect(`${token.slice(1)}x`), /401|Missing or wrong token/);
  });

  for (const [name, fn] of checks) {
    await fn();
    console.log(`ok - ${name}`);
  }

  await client.close();
  console.log(`e2e passed: @modelcontextprotocol/sdk ${sdkVersion} client against ${url.origin}, protocol ${transport.protocolVersion}`);
} finally {
  server.stdin.end();
  await once(server, 'exit');
  await rm(dir, { recursive: true, force: true });
}
