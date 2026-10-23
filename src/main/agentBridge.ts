import type { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js';
import type { WebContents } from 'electron';
import { AGENT_TOOLS, McpBridge, fail, text, type AgentResult, type AgentTab } from './agentServer';

export { DEFAULT_AGENT_PORT, type AgentTab } from './agentServer';

export interface ConsoleEntry {
  level: string;
  message: string;
  source: string;
  time: number;
}

/** What the bridge needs from the browser. Tab ids are Flecha's own tab ids. */
export interface AgentHost {
  listTabs(): AgentTab[];
  /** The tab's page, loaded if needed; the active tab when no id is given. */
  contentsFor(tabId?: string): { id: string; wc: WebContents } | undefined;
  openTab(url: string, background: boolean): string;
  activate(tabId: string): void;
  close(tabId: string): void;
  consoleLog(tabId: string): ConsoleEntry[];
  /** Shows the user, in the sidebar, what the agent is doing and where. */
  showActivity(tabId: string | undefined, action: string): void;
}

type Result = AgentResult;

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

/**
 * The agent bridge for the Electron spike: agentServer.ts's MCP server, with tools that
 * read pages and console logs, take screenshots, click, type and run JavaScript in WebContents.
 */
export class AgentBridge extends McpBridge {
  constructor(host: AgentHost, token: () => string) {
    super((mcp) => registerTools(mcp, host), token);
  }
}

function registerTools(mcp: McpServer, host: AgentHost) {
  const withTab = async (id: string | undefined, action: string, run: (wc: WebContents, id: string) => Promise<Result>) => {
    const target = host.contentsFor(id);
    if (!target) return fail(id ? `No tab with id ${id}` : 'There is no active tab');
    host.showActivity(target.id, action);
    try {
      return await run(target.wc, target.id);
    } catch (err) {
      return fail(err instanceof Error ? err.message : String(err));
    }
  };

  mcp.registerTool(
    'list_tabs',
    AGENT_TOOLS.list_tabs,
    async () => {
      host.showActivity(undefined, 'listing tabs');
      return text(host.listTabs());
    },
  );

  mcp.registerTool(
    'open_tab',
    AGENT_TOOLS.open_tab,
    async ({ url, background }) => {
      const id = host.openTab(url, !!background);
      host.showActivity(id, 'opening a tab');
      const target = host.contentsFor(id);
      if (target) await settle(target.wc);
      return text({ tabId: id, url: target?.wc.getURL() ?? url });
    },
  );

  mcp.registerTool(
    'activate_tab',
    AGENT_TOOLS.activate_tab,
    async ({ tabId: id }) => {
      host.activate(id);
      host.showActivity(id, 'switching tab');
      return text('ok');
    },
  );

  mcp.registerTool(
    'close_tab',
    AGENT_TOOLS.close_tab,
    async ({ tabId: id }) => {
      host.showActivity(id, 'closing a tab');
      host.close(id);
      return text('ok');
    },
  );

  mcp.registerTool(
    'navigate',
    AGENT_TOOLS.navigate,
    async ({ tabId: id, url, action }) =>
      withTab(id, url ? `opening ${url}` : (action ?? 'navigating'), async (wc) => {
        if (url) await wc.loadURL(url).catch(() => {});
        else if (action === 'back') wc.navigationHistory.goBack();
        else if (action === 'forward') wc.navigationHistory.goForward();
        else if (action === 'reload') wc.reload();
        await settle(wc);
        return text({ url: wc.getURL(), title: wc.getTitle() });
      }),
  );

  mcp.registerTool(
    'read_page',
    AGENT_TOOLS.read_page,
    async ({ tabId: id, format = 'text', maxChars = 40000 }) =>
      withTab(id, 'reading the page', async (wc) => {
        const code = {
          text: 'document.body?.innerText ?? ""',
          html: 'document.documentElement.outerHTML',
          links: '[...document.links].map((a) => `${a.innerText.trim().slice(0, 80)} -> ${a.href}`).join("\\n")',
        }[format];
        const content = String(await wc.executeJavaScript(code));
        return text(`# ${wc.getTitle()}\n${wc.getURL()}\n\n${content.slice(0, maxChars)}`);
      }),
  );

  mcp.registerTool(
    'screenshot',
    AGENT_TOOLS.screenshot,
    async ({ tabId: id }) =>
      withTab(id, 'taking a screenshot', async (wc, tab) => {
        host.activate(tab);
        await wait(300);
        let image = await wc.capturePage();
        if (image.getSize().width > 1280) image = image.resize({ width: 1280 });
        return { content: [{ type: 'image', data: image.toPNG().toString('base64'), mimeType: 'image/png' }] };
      }),
  );

  mcp.registerTool(
    'console_logs',
    AGENT_TOOLS.console_logs,
    async ({ tabId: id, level, limit = 50 }) =>
      withTab(id, 'reading the console', async (_wc, tab) => {
        const entries = host.consoleLog(tab).filter((e) => !level || e.level === level);
        return text(entries.slice(-limit).map((e) => `[${e.level}] ${e.message}${e.source ? `  (${e.source})` : ''}`).join('\n') || '(no messages)');
      }),
  );

  mcp.registerTool(
    'click',
    AGENT_TOOLS.click,
    async ({ tabId: id, selector }) =>
      withTab(id, `clicking ${selector}`, async (wc, tab) => {
        host.activate(tab);
        const point = await centerOf(wc, selector);
        if (!point) return fail(`No visible element matches ${selector}`);
        for (const type of ['mouseDown', 'mouseUp'] as const) {
          wc.sendInputEvent({ type, x: point.x, y: point.y, button: 'left', clickCount: 1 });
        }
        await settle(wc);
        return text({ clicked: selector, url: wc.getURL() });
      }),
  );

  mcp.registerTool(
    'type',
    AGENT_TOOLS.type,
    async ({ tabId: id, selector, text: value, clear, submit }) =>
      withTab(id, `typing into ${selector ?? 'the page'}`, async (wc, tab) => {
        host.activate(tab);
        if (selector) {
          const found = await wc.executeJavaScript(`(() => {
            const el = document.querySelector(${JSON.stringify(selector)});
            if (!el) return false;
            el.scrollIntoView({ block: 'center' });
            el.focus();
            if (${!!clear} && 'value' in el) { el.value = ''; el.dispatchEvent(new Event('input', { bubbles: true })); }
            return true;
          })()`);
          if (!found) return fail(`No element matches ${selector}`);
        }
        await wc.insertText(value);
        if (submit) pressKey(wc, 'Enter');
        await settle(wc);
        return text('ok');
      }),
  );

  mcp.registerTool(
    'press_key',
    AGENT_TOOLS.press_key,
    async ({ tabId: id, key }) =>
      withTab(id, `pressing ${key}`, async (wc, tab) => {
        host.activate(tab);
        pressKey(wc, key);
        await settle(wc);
        return text('ok');
      }),
  );

  mcp.registerTool(
    'evaluate',
    AGENT_TOOLS.evaluate,
    async ({ tabId: id, expression }) =>
      withTab(id, 'running JavaScript', async (wc) => {
        const result = await wc.executeJavaScript(expression, true);
        return text(result === undefined ? 'undefined' : result);
      }),
  );

  mcp.registerTool(
    'wait_for',
    AGENT_TOOLS.wait_for,
    async ({ tabId: id, selector, timeoutMs = 10000 }) =>
      withTab(id, `waiting for ${selector ?? 'the page'}`, async (wc) => {
        const deadline = Date.now() + timeoutMs;
        while (Date.now() < deadline) {
          const ready = selector
            ? await wc.executeJavaScript(`!!document.querySelector(${JSON.stringify(selector)})`)
            : !wc.isLoading();
          if (ready) return text('ready');
          await wait(200);
        }
        return fail(`Timed out after ${timeoutMs}ms`);
      }),
  );

}

/** Waits for navigation the last action may have started, up to a few seconds. */
async function settle(wc: WebContents) {
  await wait(150);
  const deadline = Date.now() + 8000;
  while (wc.isLoading() && Date.now() < deadline) await wait(100);
}

async function centerOf(wc: WebContents, selector: string): Promise<{ x: number; y: number } | undefined> {
  return wc.executeJavaScript(`(() => {
    const el = document.querySelector(${JSON.stringify(selector)});
    if (!el) return undefined;
    el.scrollIntoView({ block: 'center', inline: 'center' });
    const r = el.getBoundingClientRect();
    if (!r.width || !r.height) return undefined;
    return { x: Math.round(r.left + r.width / 2), y: Math.round(r.top + r.height / 2) };
  })()`);
}

function pressKey(wc: WebContents, key: string) {
  wc.sendInputEvent({ type: 'keyDown', keyCode: key });
  if (key.length === 1 || key === 'Enter') wc.sendInputEvent({ type: 'char', keyCode: key === 'Enter' ? '\r' : key });
  wc.sendInputEvent({ type: 'keyUp', keyCode: key });
}
