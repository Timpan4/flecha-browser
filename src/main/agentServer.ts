import http from 'node:http';
import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import { StreamableHTTPServerTransport } from '@modelcontextprotocol/sdk/server/streamableHttp.js';
import { z } from 'zod';

/**
 * The agent bridge's HTTP side and its tools' names, descriptions and inputs, shared by the
 * Electron spike (agentBridge.ts, which drives WebContents) and the Chromium build's native
 * messaging host (which forwards each call to the shell extension). Nothing here needs Electron.
 */

export const DEFAULT_AGENT_PORT = 7390;

export interface AgentTab {
  id: string;
  title: string;
  url: string;
  space: string;
  zone: 'favorites' | 'pinned' | 'today';
  active: boolean;
}

export type AgentResult = {
  content: Array<{ type: 'text'; text: string } | { type: 'image'; data: string; mimeType: string }>;
  isError?: boolean;
};

export const text = (value: unknown): AgentResult => ({
  content: [{ type: 'text', text: typeof value === 'string' ? value : JSON.stringify(value, null, 2) }],
});
export const fail = (message: string): AgentResult => ({ content: [{ type: 'text', text: message }], isError: true });

const tabId = z.string().optional().describe('Flecha tab id from list_tabs; defaults to the active tab');

/** Every tool a coding agent gets, by name. */
export const AGENT_TOOLS = {
  list_tabs: { description: 'List open tabs in Flecha with their ids, titles, URLs, Space and whether they are active.' },
  open_tab: {
    description: 'Open a URL in a new tab and return its id.',
    inputSchema: { url: z.string(), background: z.boolean().optional().describe('Open without switching to it') },
  },
  activate_tab: { description: 'Bring a tab on screen.', inputSchema: { tabId: z.string() } },
  close_tab: { description: 'Close a tab (pinned tabs and favorites are unloaded instead).', inputSchema: { tabId: z.string() } },
  navigate: {
    description: 'Load a URL in a tab, or go back/forward/reload, and wait for the page to settle.',
    inputSchema: { tabId, url: z.string().optional(), action: z.enum(['back', 'forward', 'reload']).optional() },
  },
  read_page: {
    description: "Read a tab's content: visible text (default), full HTML, or its links.",
    inputSchema: { tabId, format: z.enum(['text', 'html', 'links']).optional(), maxChars: z.number().optional() },
  },
  screenshot: { description: 'Screenshot a tab (brings it on screen first).', inputSchema: { tabId } },
  console_logs: {
    description: "A tab's recent console messages (errors, warnings, logs), newest last.",
    inputSchema: { tabId, level: z.enum(['error', 'warning', 'info', 'debug']).optional(), limit: z.number().optional() },
  },
  click: {
    description: 'Click the element matching a CSS selector, with a real mouse event.',
    inputSchema: { tabId, selector: z.string() },
  },
  type: {
    description: 'Type text into the element matching a selector (or the focused one), optionally clearing it first and pressing Enter.',
    inputSchema: { tabId, selector: z.string().optional(), text: z.string(), clear: z.boolean().optional(), submit: z.boolean().optional() },
  },
  press_key: { description: 'Press a key such as Enter, Tab, Escape or ArrowDown in a tab.', inputSchema: { tabId, key: z.string() } },
  evaluate: {
    description: "Run JavaScript in a tab's page and return the result (promises are awaited, values are JSON-serialized).",
    inputSchema: { tabId, expression: z.string() },
  },
  wait_for: {
    description: 'Wait until an element matching a selector exists (or the page finishes loading when no selector is given).',
    inputSchema: { tabId, selector: z.string().optional(), timeoutMs: z.number().optional() },
  },
} as const;

export type AgentToolName = keyof typeof AGENT_TOOLS;

/**
 * An MCP server (Streamable HTTP) that lets coding agents see and drive Flecha's tabs.
 *
 * It only listens on 127.0.0.1, only answers requests whose Host is local, and every
 * request needs the bearer token shown in Settings. `register` adds the tools.
 */
export class McpBridge {
  private server?: http.Server;
  port?: number;

  constructor(
    private register: (mcp: McpServer) => void,
    private token: () => string,
  ) {}

  get running() {
    return !!this.server;
  }

  async start(preferredPort = DEFAULT_AGENT_PORT) {
    if (this.server) return;
    for (let port = preferredPort; port < preferredPort + 10; port++) {
      const server = http.createServer((req, res) => void this.handle(req, res));
      const ok = await new Promise<boolean>((resolve) => {
        server.once('error', () => resolve(false));
        server.listen(port, '127.0.0.1', () => resolve(true));
      });
      if (ok) {
        this.server = server;
        this.port = port;
        return;
      }
    }
    throw new Error('No free port for the agent bridge');
  }

  async stop() {
    const server = this.server;
    this.server = undefined;
    this.port = undefined;
    if (server) await new Promise((resolve) => server.close(resolve));
  }

  private async handle(req: http.IncomingMessage, res: http.ServerResponse) {
    const hostname = (req.headers.host ?? '').replace(/:\d+$/, '');
    if (!['127.0.0.1', 'localhost'].includes(hostname)) return reply(res, 403, 'Forbidden host');
    const token = this.token();
    if (!token || req.headers.authorization !== `Bearer ${token}`) return reply(res, 401, 'Missing or wrong token');
    if (!req.url?.startsWith('/mcp')) return reply(res, 404, 'Not found');

    // Stateless: a fresh server and transport per request, as the SDK recommends.
    const mcp = new McpServer({ name: 'flecha', version: '0.1.0' });
    this.register(mcp);
    const transport = new StreamableHTTPServerTransport({ sessionIdGenerator: undefined });
    res.on('close', () => {
      void transport.close();
      void mcp.close();
    });
    await mcp.connect(transport);
    await transport.handleRequest(req, res, req.method === 'POST' ? await readJson(req) : undefined);
  }
}

function reply(res: http.ServerResponse, status: number, message: string) {
  res.writeHead(status, { 'content-type': 'text/plain' }).end(message);
}

function readJson(req: http.IncomingMessage): Promise<unknown> {
  return new Promise((resolve, reject) => {
    let body = '';
    req.setEncoding('utf8');
    req.on('data', (chunk) => (body += chunk));
    req.on('end', () => {
      try {
        resolve(body ? JSON.parse(body) : undefined);
      } catch (err) {
        reject(err);
      }
    });
    req.on('error', reject);
  });
}
