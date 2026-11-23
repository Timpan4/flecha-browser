// Records what today's agent bridge advertises and accepts, for the Rust port's parity test.
//
// The tools come from chromium/shared/agentTools.ts, served by the MCP SDK the native host uses.
// Every argument case in CASES runs through the SDK's own validation, so the fixture holds zod's
// verdict and message for each one.
//
//   node rewrite/crates/flecha-services/scripts/agent-tools-fixture.mjs          writes the fixture
//   node rewrite/crates/flecha-services/scripts/agent-tools-fixture.mjs --check  fails if it is stale
import { readFile, writeFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { Client } from '@modelcontextprotocol/sdk/client/index.js';
import { InMemoryTransport } from '@modelcontextprotocol/sdk/inMemory.js';
import { LATEST_PROTOCOL_VERSION, SUPPORTED_PROTOCOL_VERSIONS } from '@modelcontextprotocol/sdk/types.js';
import { loadModule } from '../../../../chromium/tests/load-module.mjs';

const fixturePath = fileURLToPath(new URL('../tests/fixtures/agent-tools.json', import.meta.url));

// The SDK's export map serves a stub package.json, so read the installed one.
const sdkVersion = JSON.parse(await readFile(new URL('../../../../node_modules/@modelcontextprotocol/sdk/package.json', import.meta.url), 'utf8')).version;

const { AGENT_TOOLS } = await loadModule('chromium/shared/agentTools.ts');
const { TOOL_TIMEOUT, MAX_TOOL_WAIT } = await loadModule('chromium/shared/nativeMessaging.ts');

// Arguments as raw JSON text, so number spellings such as 10000.0 reach both parsers unchanged.
// `null` sends the call without an `arguments` member.
const CASES = [
  ['list_tabs', null],
  ['list_tabs', '{}'],
  ['list_tabs', '{"tabId":5}'],
  ['open_tab', '{"url":"https://example.com/"}'],
  ['open_tab', '{"url":"example.com","extra":true}'],
  ['open_tab', '{}'],
  ['open_tab', null],
  ['open_tab', '{"url":null}'],
  ['open_tab', '{"url":1}'],
  ['open_tab', '{"url":["a"]}'],
  ['open_tab', '{"url":{"href":"a"}}'],
  ['activate_tab', '{"tabId":"t1"}'],
  ['activate_tab', '{"tabId":1}'],
  ['activate_tab', '{"tabId":"t1","tabId":2}'],
  ['activate_tab', '{"tabId":"t1","__proto__":{"polluted":true}}'],
  ['close_tab', '{"tabId":"t1"}'],
  ['close_tab', '{"tabId":false}'],
  ['navigate', '{"tabId":"t1"}'],
  ['navigate', '{"tabId":"t1","url":"https://example.com/"}'],
  ['navigate', '{"tabId":"t1","action":"back"}'],
  ['navigate', '{"tabId":"t1","action":"reload","url":"x"}'],
  ['navigate', '{"tabId":"t1","action":"up"}'],
  ['navigate', '{"tabId":"t1","action":"BACK"}'],
  ['navigate', '{"tabId":"t1","action":null}'],
  ['navigate', '{"tabId":"t1","url":null}'],
  ['navigate', '{"tabId":"t1","action":1}'],
  ['read_page', '{"tabId":"t1"}'],
  ['read_page', '{"tabId":"t1","format":"links","maxChars":10}'],
  ['read_page', '{"tabId":"t1","format":"html","maxChars":2.5}'],
  ['read_page', '{"tabId":"t1","maxChars":-3}'],
  ['read_page', '{"tabId":"t1","maxChars":1e300}'],
  ['read_page', '{"tabId":"t1","maxChars":-0.0}'],
  ['read_page', '{"tabId":"t1","maxChars":"10"}'],
  ['read_page', '{"tabId":"t1","maxChars":null}'],
  ['read_page', '{"tabId":"t1","format":"pdf"}'],
  ['read_page', '{"tabId":"t1","format":null}'],
  ['screenshot', '{"tabId":"t1"}'],
  ['screenshot', '{"tabId":null}'],
  ['console_logs', '{"tabId":"t1","level":"warning","limit":5}'],
  ['console_logs', '{"tabId":"t1","level":"warn"}'],
  ['console_logs', '{"tabId":"t1","limit":-1}'],
  ['console_logs', '{"tabId":"t1","limit":0.5}'],
  ['console_logs', '{"tabId":"t1","limit":true}'],
  ['click', '{"tabId":"t1","selector":"#go"}'],
  ['click', '{"tabId":5,"selector":5}'],
  ['click', '{"tabId":"t1"}'],
  ['type', '{"tabId":"t1","text":"hi","clear":true,"submit":false}'],
  ['type', '{"tabId":"t1","selector":"input","text":""}'],
  ['type', '{"tabId":"t1","text":"hi","clear":"yes"}'],
  ['type', '{"tabId":"t1","text":"hi","clear":0}'],
  ['type', '{"tabId":"t1","selector":null,"text":"hi"}'],
  ['type', '{"tabId":"t1"}'],
  ['press_key', '{"tabId":"t1","key":"Enter"}'],
  ['press_key', '{"tabId":"t1","key":13}'],
  ['evaluate', '{"tabId":"t1","expression":"1+1"}'],
  ['evaluate', '{"tabId":"t1","expression":5}'],
  ['evaluate', '{}'],
  ['wait_for', '{"tabId":"t1"}'],
  ['wait_for', '{"tabId":"t1","selector":"#done","timeoutMs":0}'],
  ['wait_for', '{"tabId":"t1","timeoutMs":10000}'],
  ['wait_for', '{"tabId":"t1","timeoutMs":10000.0}'],
  ['wait_for', '{"tabId":"t1","timeoutMs":1e4}'],
  ['wait_for', '{"tabId":"t1","timeoutMs":-0.0}'],
  ['wait_for', `{"tabId":"t1","timeoutMs":${MAX_TOOL_WAIT}}`],
  ['wait_for', `{"tabId":"t1","timeoutMs":${MAX_TOOL_WAIT + 1}}`],
  ['wait_for', '{"tabId":"t1","timeoutMs":-1}'],
  ['wait_for', '{"tabId":"t1","timeoutMs":1.5}'],
  ['wait_for', '{"tabId":"t1","timeoutMs":-1.5}'],
  ['wait_for', '{"tabId":"t1","timeoutMs":9007199254740993}'],
  ['wait_for', '{"tabId":"t1","timeoutMs":"5"}'],
  ['wait_for', '{"tabId":"t1","timeoutMs":null}'],
  ['wait_for', '{"tabId":"t1","selector":null}'],
];

async function record() {
  const server = new McpServer({ name: 'flecha', version: '0.1.0' });

  for (const [name, spec] of Object.entries(AGENT_TOOLS)) {
    // As host.ts registers them: a tool without inputSchema runs with no arguments.
    server.registerTool(name, spec, async (args) => ({
      content: [{ type: 'text', text: JSON.stringify('inputSchema' in spec ? args : {}) }],
    }));
  }

  const [serverSide, clientSide] = InMemoryTransport.createLinkedPair();
  await server.connect(serverSide);
  const client = new Client({ name: 'agent-tools-fixture', version: '1' });
  await client.connect(clientSide);

  const { tools } = await client.listTools();
  const validation = [];

  for (const [tool, args] of CASES) {
    const request = { name: tool };

    if (args !== null) request.arguments = JSON.parse(args);

    const result = await client.callTool(request);
    const [content] = result.content;

    validation.push(
      result.isError
        ? { tool, arguments: args, ok: false, error: content.text }
        : { tool, arguments: args, ok: true, parsed: JSON.parse(content.text) },
    );
  }

  await client.close();
  await server.close();

  return {
    source: `chromium/shared/agentTools.ts through @modelcontextprotocol/sdk ${sdkVersion}; regenerate with scripts/agent-tools-fixture.mjs`,
    serverInfo: client.getServerVersion(),
    capabilities: client.getServerCapabilities(),
    protocolVersions: { latest: LATEST_PROTOCOL_VERSION, supported: SUPPORTED_PROTOCOL_VERSIONS },
    constants: { TOOL_TIMEOUT, MAX_TOOL_WAIT },
    tools,
    validation,
  };
}

const text = JSON.stringify(await record(), null, 2) + '\n';

if (process.argv.includes('--check')) {
  const committed = await readFile(fixturePath, 'utf8').catch(() => '');

  if (committed !== text) {
    console.error(`${fixturePath} is stale: run node rewrite/crates/flecha-services/scripts/agent-tools-fixture.mjs`);
    process.exit(1);
  }
} else {
  await writeFile(fixturePath, text);
}
