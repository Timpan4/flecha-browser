import { z } from 'zod';
import { MAX_TOOL_WAIT } from '../native-host/protocol';

/**
 * The agent bridge's tools: names, descriptions and inputs, as the Electron version had
 * them: the native host serves them and the shell extension runs them. Needs no Node.
 */

export interface AgentTab {
  id: string;
  title: string;
  url: string;
  space: string;
  zone: 'favorites' | 'pinned' | 'today';
  active: boolean;
  /** In the agent's folder: a tab an agent opened. */
  agent: boolean;
}

export type AgentResult = {
  content: Array<{ type: 'text'; text: string } | { type: 'image'; data: string; mimeType: string }>;
  isError?: boolean;
};

export const text = (value: unknown): AgentResult => ({
  content: [{ type: 'text', text: typeof value === 'string' ? value : JSON.stringify(value, null, 2) }],
});
export const fail = (message: string): AgentResult => ({ content: [{ type: 'text', text: message }], isError: true });

/**
 * Required: several agents can share the bridge, which can't tell them apart, so no default tab
 * would be the same one for each.
 */
const tabId = z.string().describe('Flecha tab id, from open_tab or list_tabs');
const allowVisibleTab = z.boolean().optional().describe('Set to true only when the user asks you to act on a tab they are viewing.');

/**
 * Tools work in the background by default. Activating a tab and editing a visible
 * page are explicit operations.
 */
export const AGENT_TOOLS = {
  list_tabs: {
    description:
      'List open tabs in Flecha with their ids, titles, URLs and Space. `active` marks the tabs the user is looking at; `agent` marks tabs coding agents opened.',
  },
  open_tab: {
    description:
      "Open a URL in a new background tab, in the sidebar's agent folder, and return its id for the other tools. The user's view and keyboard stay where they are; the other tools work on the tab without showing it.",
    inputSchema: { url: z.string() },
  },
  activate_tab: {
    description: "Switch the user's view to a tab. Only when the user asks to see it: the other tools don't need the tab on screen.",
    inputSchema: { tabId: z.string() },
  },
  close_tab: { description: 'Close a tab (pinned tabs and favorites are unloaded instead).', inputSchema: { tabId, allowVisibleTab } },
  navigate: {
    description: 'Load a URL in a tab, or go back/forward/reload, and wait for the page to settle.',
    inputSchema: { tabId, allowVisibleTab, url: z.string().optional(), action: z.enum(['back', 'forward', 'reload']).optional() },
  },
  read_page: {
    description: "Read a tab's content: visible text (default), full HTML, or its links.",
    inputSchema: { tabId, format: z.enum(['text', 'html', 'links']).optional(), maxChars: z.number().optional() },
  },
  screenshot: { description: "Screenshot a tab's viewport, in the background.", inputSchema: { tabId } },
  console_logs: {
    description: "A tab's recent console messages (errors, warnings, logs), newest last.",
    inputSchema: { tabId, level: z.enum(['error', 'warning', 'info', 'debug']).optional(), limit: z.number().optional() },
  },
  click: {
    description: 'Click the element matching a CSS selector, with a real mouse event.',
    inputSchema: { tabId, allowVisibleTab, selector: z.string() },
  },
  type: {
    description: 'Type text into the element matching a selector (or the focused one), optionally clearing it first and pressing Enter.',
    inputSchema: { tabId, allowVisibleTab, selector: z.string().optional(), text: z.string(), clear: z.boolean().optional(), submit: z.boolean().optional() },
  },
  press_key: { description: 'Press a key such as Enter, Tab, Escape or ArrowDown in a tab.', inputSchema: { tabId, allowVisibleTab, key: z.string() } },
  evaluate: {
    description: "Run JavaScript in a tab's page and return the result (promises are awaited, values are JSON-serialized). If an evaluation times out while a promise is pending, further edits wait for it to finish or for the page to unload.",
    inputSchema: { tabId, allowVisibleTab, expression: z.string() },
  },
  wait_for: {
    description: 'Wait until an element matching a selector exists (or the page finishes loading when no selector is given).',
    inputSchema: { tabId, selector: z.string().optional(), timeoutMs: z.number().int().min(0).max(MAX_TOOL_WAIT).optional() },
  },
} as const;

export type AgentToolName = keyof typeof AGENT_TOOLS;
export type AgentToolArgs = {
  [Name in AgentToolName]: (typeof AGENT_TOOLS)[Name] extends { inputSchema: infer Shape extends z.ZodRawShape }
    ? z.infer<z.ZodObject<Shape>>
    : Record<string, never>;
};
