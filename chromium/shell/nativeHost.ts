/**
 * The shell's side of Flecha's native messaging host (chromium/native-host): importing from
 * other browsers, reading the Electron spike's state, and the agent bridge for coding agents.
 *
 * The host reads files and serves MCP on 127.0.0.1; this side adds imports to the Store and
 * runs agents' tool calls on real tabs with chrome.tabs and chrome.debugger. The port to the
 * host opens on first use and stays open while the agent bridge is on (which also keeps the
 * background awake); otherwise it closes after a minute of quiet.
 *
 * Install the host with chromium/scripts/install-native-host.sh.
 */
import type { DetectedBrowser, HistoryEntry, ImportOptions, ImportPreview, ImportResult, UiState } from '../shared/types';
import { pinnedTabIds, type Store } from '../shared/store';
import type { AgentResult, AgentTab, AgentToolArgs, AgentToolName } from '../shared/agentTools';
import { applyBrowserImport, importOptions, previewBrowserImport, type BrowserImportData } from '../shared/importApply';
import { HOST_NAME, type HostMethods, type ToHost, type ToShell } from '../native-host/protocol';
import { broadcast, handle } from './hub';

export interface NativeHostContext {
  store: Store;
  /** This build can give a Space its own cookies and logins (Separate Cookies and Logins). */
  canIsolate: boolean;
  /** Sends every page the new state (the sidebar shows what an agent is doing). */
  broadcastState(): void;
  tabs: {
    /** Opens an address in a new background Today tab, in the agent's folder; returns its id. */
    open(url: string): string;
    /** The Chromium tab showing a Flecha tab, opened (in the background) if it has none yet. */
    chromeTab(tabId: string, requireCurrent: () => void): Promise<number | undefined>;
    discardUnopenedAgentTab(tabId: string): void;
    currentChromeTab(tabId: string): number | undefined;
    activate(tabId: string): void;
    close(tabId: string, allowVisible: boolean, check: (chromeId: number | undefined) => Promise<void>): Promise<void>;
  };
  /** The agent started or stopped working in a tab (`agentTabsInUse` changed): tab sleep wakes it and lets it be. Settles once woken. */
  agentTabsChanged(): Promise<void>;
}

const NOT_INSTALLED =
  'Flecha’s helper is unavailable, so it can’t import from other browsers or run the agent bridge. Repair or reinstall Flecha, then try again.';
const IDLE_CLOSE = 60_000;
/** How long the agent bridge waits before starting the host again, doubling up to a minute. */
const RETRY_FIRST = 2_000;
const RETRY_MAX = 60_000;

/** Chromium's reasons for not starting a host at all (native_message_host.cc), as opposed to one that stopped. */
const isMissing = (reason: string) =>
  /host not found|host is forbidden|invalid native messaging host|failed to start native messaging host/i.test(reason);

/** Why the helper couldn't be used, in words for the person importing. */
function helperProblem(err: unknown) {
  const message = String(err instanceof Error ? err.message : err);
  return isMissing(message) ? NOT_INSTALLED : message;
}

export function registerNativeHost(ctx: NativeHostContext) {
  const { store } = ctx;

  // ---- the port ---------------------------------------------------------------

  let port: chrome.runtime.Port | undefined;
  let nextCall = 1;
  const calls = new Map<number, { resolve(value: unknown): void; reject(err: Error): void }>();
  const chunks = new Map<number, string[]>();
  let idleTimer: ReturnType<typeof setTimeout> | undefined;
  let sentAgent = '';
  /** Why the host last failed, for Settings; cleared once it answers. */
  let hostError: string | undefined;
  /** Why the running host couldn't start the bridge; only the bridge's next report clears it. */
  let agentError: string | undefined;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let retryDelay = RETRY_FIRST;

  /** Tells the sidebar and an open Settings page that the host or bridge changed. */
  function changed() {
    ctx.broadcastState();
    broadcast('settings:changed');
  }

  function setHostError(error: string | undefined) {
    if (error === hostError) return;
    hostError = error;
    changed();
  }

  function connect() {
    if (port) return port;
    const p = chrome.runtime.connectNative(HOST_NAME);
    port = p;
    sentAgent = '';
    p.onMessage.addListener((message: ToShell) => receive(message));
    p.onDisconnect.addListener(() => {
      const reason = chrome.runtime.lastError?.message ?? 'The helper stopped';
      if (port === p) port = undefined;
      endpoint = undefined;
      agentError = undefined;
      for (const call of calls.values()) call.reject(new Error(reason));
      calls.clear();
      chunks.clear();
      console.warn('Flecha shell: native host disconnected:', reason);
      hostError = isMissing(reason) ? NOT_INSTALLED : `Flecha’s helper stopped: ${reason}`;
      changed();
      // The bridge should stay up while it's on: start the host again, a little later each time.
      if (store.state.agent?.enabled) {
        clearTimeout(retryTimer);
        retryTimer = setTimeout(() => {
          sentAgent = '';
          syncAgent();
        }, retryDelay);
        retryDelay = Math.min(retryDelay * 2, RETRY_MAX);
      }
    });
    return p;
  }

  /** Closes the port after a quiet minute, unless the agent bridge needs it. */
  function touch() {
    clearTimeout(idleTimer);
    idleTimer = setTimeout(() => {
      if (!store.state.agent?.enabled && !calls.size) {
        port?.disconnect();
        port = undefined;
      }
    }, IDLE_CLOSE);
  }

  function post(message: ToHost) {
    connect().postMessage(message);
    touch();
  }

  function call<K extends keyof HostMethods>(method: K, ...params: Parameters<HostMethods[K]>): Promise<ReturnType<HostMethods[K]>> {
    return new Promise((resolve, reject) => {
      const id = nextCall++;
      calls.set(id, { resolve: resolve as (value: unknown) => void, reject });
      try {
        post({ type: 'call', id, method, params });
      } catch (err) {
        calls.delete(id);
        reject(err instanceof Error ? err : new Error(String(err)));
      }
    });
  }

  function receive(message: ToShell) {
    touch();
    retryDelay = RETRY_FIRST;
    if (message.type === 'chunk') {
      const parts = chunks.get(message.id) ?? [];
      parts[message.index] = message.data;
      chunks.set(message.id, parts);
      if (parts.filter((p) => p !== undefined).length < message.total) return;
      chunks.delete(message.id);
      return receive(JSON.parse(parts.join('')) as ToShell);
    }
    if (message.type === 'reply') {
      setHostError(undefined);
      const pending = calls.get(message.id);
      calls.delete(message.id);
      if (message.error) pending?.reject(new Error(message.error));
      else pending?.resolve(message.value);
    } else if (message.type === 'agent') {
      endpoint = message.endpoint;
      hostError = undefined;
      agentError = message.error ? `The agent bridge couldn’t start: ${message.error}` : undefined;
      if (message.error) console.error('Flecha shell: agent bridge failed:', message.error);
      changed();
    } else if (message.type === 'tool') {
      void runTool(message.name as AgentToolName, (message.args ?? {}) as AgentToolArgs[AgentToolName], message.deadline).then((result) => {
        try {
          post({ type: 'toolResult', id: message.id, result });
        } catch {
          // The host went away; the agent's request went with it.
        }
      });
    }
  }

  // ---- importing --------------------------------------------------------------

  /**
   * Chromium's history API can add a page but not when it was visited or its title, so
   * imported pages count as visited now; the address bar still finds them.
   */
  const history = {
    async merge(entries: HistoryEntry[]) {
      const urls = [...new Set(entries.filter((e) => /^https?:/.test(e.url)).map((e) => e.url))];
      // Oldest first, so the most recent visit ends up newest.
      urls.reverse();
      for (let i = 0; i < urls.length; i += 50) {
        await Promise.all(urls.slice(i, i + 50).map((url) => chrome.history.addUrl({ url }).catch(() => undefined)));
      }
      return urls.length;
    },
  };

  /** What an import read by `read` would bring over; `name` stands in for the source when the helper fails. */
  async function preview(name: string, read: () => Promise<BrowserImportData>): Promise<ImportPreview> {
    try {
      return previewBrowserImport(await read(), store.state, ctx.canIsolate);
    } catch (err) {
      return { name, spaces: [], favorites: 0, archived: 0, history: 0, skipped: [], canIsolate: ctx.canIsolate, warnings: [helperProblem(err)], spacesKey: '' };
    }
  }

  async function apply(options: ImportOptions, read: () => Promise<BrowserImportData>): Promise<ImportResult> {
    try {
      return await applyBrowserImport(await read(), importOptions(options, ctx.canIsolate), store, history);
    } catch (err) {
      return { spaces: 0, merged: 0, bookmarks: 0, history: 0, archived: 0, warnings: [helperProblem(err)], details: [] };
    }
  }

  handle({
    detectBrowsers: async (): Promise<DetectedBrowser[]> => {
      try {
        return await call('detectBrowsers');
      } catch (err) {
        console.warn('Flecha shell: browser discovery failed:', err);
        throw new Error('Browser import is unavailable because Flecha could not connect to its import helper.');
      }
    },
    previewImport: (id: string) => preview(String(id), () => call('readBrowser', String(id))),
    // The browser is read again rather than kept from the preview: the background may have slept since.
    // The preview's spacesKey makes sure the Spaces it showed are still the ones being imported.
    importBrowser: (id: string, options: ImportOptions) => apply(options, () => call('readBrowser', String(id))),
    /** A bookmarks file exported from any browser as HTML, by its text; the page sends it again to import. */
    previewBookmarksFile: (html: string, fileName: string) => preview(String(fileName), () => call('readBookmarksHtml', String(html), String(fileName))),
    importBookmarksFile: (html: string, fileName: string, options: ImportOptions) =>
      apply(options, () => call('readBookmarksHtml', String(html), String(fileName))),
    /** The Electron spike's saved state as text, or undefined when there's none or no helper. */
    'nativeHost:flechaState': async () => call('readFlechaState').catch(() => undefined),
  });

  // ---- the agent bridge ---------------------------------------------------------

  let endpoint: string | undefined;
  let activity: { tabId?: string; action: string; at: number } | undefined;
  let activityTimer: ReturnType<typeof setTimeout> | undefined;

  /** Starts, stops or re-keys the bridge whenever Settings change it. */
  function syncAgent() {
    const agent = store.state.agent;
    const wanted = JSON.stringify({ enabled: !!agent?.enabled, token: agent?.token ?? '' });
    if (wanted === sentAgent) return;
    if (!agent?.enabled) clearTimeout(retryTimer);
    // Nothing to stop if the host isn't running.
    if (!agent?.enabled && !port) {
      sentAgent = wanted;
      return;
    }
    sentAgent = wanted;
    try {
      post({ type: 'agent', enabled: !!agent?.enabled, token: agent?.token ?? '' });
    } catch (err) {
      console.warn('Flecha shell: no agent bridge:', err);
      setHostError(NOT_INSTALLED);
    }
  }
  store.onChange(syncAgent);
  syncAgent();

  function showActivity(tabId: string | undefined, action: string) {
    activity = { tabId, action, at: Date.now() };
    ctx.broadcastState();
    // Drop the "busy" highlight once the agent has been quiet for a moment.
    clearTimeout(activityTimer);
    activityTimer = setTimeout(() => ctx.broadcastState(), 4000);
  }

  function listTabs(): AgentTab[] {
    const visible = new Set(store.state.windows.flatMap((w) => store.visibleTabIds(w.id)));
    const tabs: AgentTab[] = [];
    const add = (id: string, space: string, zone: AgentTab['zone']) => {
      const tab = store.state.tabs[id];
      if (tab) tabs.push({ id, title: tab.customTitle || tab.title, url: tab.url, space, zone, active: visible.has(id), agent: !!tab.agent });
    };
    for (const id of store.state.favorites) add(id, '', 'favorites');
    for (const space of store.state.spaces) {
      for (const id of pinnedTabIds(space.pinned)) add(id, space.name, 'pinned');
      // Today tabs of every window.
      for (const win of store.state.windows) for (const id of win.spaces[space.id]?.today ?? []) add(id, space.name, 'today');
    }
    return tabs;
  }

  async function runTool<Name extends AgentToolName>(name: Name, args: AgentToolArgs[Name], deadline?: number): Promise<AgentResult> {
    const requireCurrent = () => {
      if (deadline !== undefined && Date.now() >= deadline) throw new Error('The agent request expired.');
    };
    try {
      requireCurrent();
      if (!Object.hasOwn(tools, name)) throw new Error(`Unknown agent tool: ${name}`);
      return await tools[name](args, requireCurrent);
    } catch (err) {
      return fail(err instanceof Error ? err.message : String(err));
    }
  }

  type EditableState = { visible: boolean; discarded: boolean };
  type PageTarget = { chromeId: number; checkEditable?: () => Promise<EditableState>; requireCurrent: () => void };

  /**
   * The tool's tab with its Chromium tab, after telling the sidebar. It stays where it is: tools
   * drive tabs through chrome.debugger, which needs no tab on screen and no window focus.
   */
  async function target(tabId: string, action: string, requireCurrent: () => void): Promise<PageTarget> {
    requireCurrent();
    if (!store.state.tabs[tabId]) throw new Error(`No tab with id ${tabId}`);
    showActivity(tabId, action);
    const chromeId = await ctx.tabs.chromeTab(tabId, requireCurrent);
    requireCurrent();
    if (chromeId === undefined) throw new Error(`Tab ${tabId} couldn’t be opened`);
    return { chromeId, requireCurrent };
  }

  function requireBackgroundTab(tabId: string, allowVisibleTab: unknown, visibleInChrome = false) {
    if (allowVisibleTab !== true && (visibleInChrome || store.windowShowing(tabId) !== undefined)) {
      throw new Error('This tab is visible. Use open_tab to work in the background, or set allowVisibleTab only if the user asked you to act on this tab.');
    }
  }

  async function checkEditable(tabId: string, chromeId: number | undefined, allowVisibleTab: unknown) {
    requireBackgroundTab(tabId, allowVisibleTab);
    let visibleInChrome = false;
    let discarded = false;
    if (chromeId !== undefined) {
      // The Store can switch before Chromium finishes showing the replacement tab.
      const tabs = await chrome.tabs.query({});
      const tab = tabs.find((candidate) => candidate.id === chromeId);
      if (!tab) throw new Error('The Chromium tab has closed.');
      discarded = tab.discarded === true;
      const split = tab.splitViewId;
      visibleInChrome = tab.active || (split !== undefined && split >= 0 &&
        tabs.some((peer) => peer.active && peer.windowId === tab.windowId && peer.splitViewId === split));
      requireBackgroundTab(tabId, allowVisibleTab, visibleInChrome);
    }
    if (!store.state.tabs[tabId] || ctx.tabs.currentChromeTab(tabId) !== chromeId) {
      throw new Error('Tab ownership changed before the agent action could run.');
    }
    return { visible: visibleInChrome, discarded };
  }

  async function editTarget(tabId: string, action: string, allowVisibleTab: unknown, requireCurrent: () => void): Promise<PageTarget> {
    requireBackgroundTab(tabId, allowVisibleTab);
    const page = await target(tabId, action, requireCurrent);
    return {
      chromeId: page.chromeId,
      requireCurrent,
      checkEditable: async () => {
        requireCurrent();
        const state = await checkEditable(tabId, page.chromeId, allowVisibleTab);
        requireCurrent();
        return state;
      },
    };
  }

  const tools: { [Name in AgentToolName]: (args: AgentToolArgs[Name], requireCurrent: () => void) => Promise<AgentResult> } = {
    list_tabs: async () => {
      showActivity(undefined, 'listing tabs');
      return text(listTabs());
    },
    open_tab: async ({ url }, requireCurrent) => {
      const tabId = ctx.tabs.open(String(url));
      showActivity(tabId, 'opening a tab');
      let chromeId: number | undefined;
      try {
        chromeId = await ctx.tabs.chromeTab(tabId, requireCurrent);
      } catch (error) {
        try { requireCurrent(); }
        catch { ctx.tabs.discardUnopenedAgentTab(tabId); }
        throw error;
      }
      if (chromeId !== undefined) await settle(chromeId);
      const tab = chromeId !== undefined ? await chrome.tabs.get(chromeId).catch(() => undefined) : undefined;
      return text({ tabId, url: tab?.pendingUrl || tab?.url || url });
    },
    activate_tab: async ({ tabId }) => {
      if (!store.state.tabs[tabId]) return fail(`No tab with id ${tabId}`);
      ctx.tabs.activate(tabId);
      showActivity(tabId, 'switching tab');
      return text('ok');
    },
    close_tab: async ({ tabId, allowVisibleTab }, requireCurrent) => {
      if (!store.state.tabs[tabId]) return fail(`No tab with id ${tabId}`);
      showActivity(tabId, 'closing a tab');
      await ctx.tabs.close(tabId, allowVisibleTab === true, async (chromeId) => {
        requireCurrent();
        await checkEditable(tabId, chromeId, allowVisibleTab);
        requireCurrent();
      });
      return text('ok');
    },
    navigate: async ({ tabId, url, action, allowVisibleTab }, requireCurrent) => {
      const t = await editTarget(tabId, url ? `opening ${url}` : (action ?? 'navigating'), allowVisibleTab, requireCurrent);
      return withMutation(t, async () => {
        await t.checkEditable?.();
        if (url) await chrome.tabs.update(t.chromeId, { url });
        else if (action === 'back') await chrome.tabs.goBack(t.chromeId).catch(() => undefined);
        else if (action === 'forward') await chrome.tabs.goForward(t.chromeId).catch(() => undefined);
        else if (action === 'reload') await chrome.tabs.reload(t.chromeId);
        await settle(t.chromeId);
        const tab = await chrome.tabs.get(t.chromeId);
        return text({ url: tab.url, title: tab.title });
      });
    },
    read_page: async ({ tabId, format = 'text', maxChars = 40000 }, requireCurrent) => {
      const t = await target(tabId, 'reading the page', requireCurrent);
      const code = {
        text: 'document.body?.innerText ?? ""',
        html: 'document.documentElement.outerHTML',
        links: '[...document.links].map((a) => `${a.innerText.trim().slice(0, 80)} -> ${a.href}`).join("\\n")',
      }[format as 'text' | 'html' | 'links'];
      if (!code) return fail(`Unknown format ${format}`);
      const content = String(await evaluate(t, code));
      const tab = await chrome.tabs.get(t.chromeId);
      return text(`# ${tab.title}\n${tab.url}\n\n${content.slice(0, maxChars)}`);
    },
    screenshot: async ({ tabId }, requireCurrent) => {
      const t = await target(tabId, 'taking a screenshot', requireCurrent);
      // Chromium has a tab that isn't on screen draw a frame for this (page_handler.cc).
      const shot = (await debug(t, 'Page.captureScreenshot', { format: 'png' })) as { data: string };
      return { content: [{ type: 'image', data: await shrink(shot.data, 1280), mimeType: 'image/png' }] };
    },
    console_logs: async ({ tabId, level, limit = 50 }, requireCurrent) => {
      const t = await target(tabId, 'reading the console', requireCurrent);
      await attachTarget(t);
      const entries = (consoleLogs.get(t.chromeId) ?? []).filter((e) => !level || e.level === level);
      return text(entries.slice(-limit).map((e) => `[${e.level}] ${e.message}${e.source ? `  (${e.source})` : ''}`).join('\n') || '(no messages)');
    },
    click: async ({ tabId, selector, allowVisibleTab }, requireCurrent) => {
      const t = await editTarget(tabId, `clicking ${selector}`, allowVisibleTab, requireCurrent);
      const clicked = await withInput(t, () => withElement(t, selector, async (element) => {
        const point = (await element(`
          if (!el.isConnected || el !== document.querySelector(${JSON.stringify(selector)})) return;
          el.scrollIntoView({ block: 'center', inline: 'center' });
          const rect = el.getBoundingClientRect();
          const x = Math.round(rect.left + rect.width / 2);
          const y = Math.round(rect.top + rect.height / 2);
          if (rect.width && rect.height && el.contains(document.elementFromPoint(x, y))) return { x, y };
        `)) as { x: number; y: number } | undefined;
        if (!point) return false;
        const send = await debuggerFor(t);
        await settleGesture(['mousePressed', 'mouseReleased'].map((type) =>
          send('Input.dispatchMouseEvent', { type, x: point.x, y: point.y, button: 'left', clickCount: 1 }),
        ));
        return true;
      }));
      if (!clicked) return fail(`No visible element matches ${selector}`);
      await settle(t.chromeId);
      return text({ clicked: selector, url: (await chrome.tabs.get(t.chromeId)).url });
    },
    type: async ({ tabId, selector, text: value, clear, submit, allowVisibleTab }, requireCurrent) => {
      const t = await editTarget(tabId, `typing into ${selector ?? 'the page'}`, allowVisibleTab, requireCurrent);
      const input = value;
      const typeInto = async (element?: ElementCall) => {
        let replaced = false;
        if (element) {
          const result = await element(`
              const uneditable = () => el.matches(':disabled') ||
                ((el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) && el.readOnly);
              if (uneditable()) return 'readonly';
              el.scrollIntoView({ block: 'center' });
              el.focus();
              if (document.activeElement !== el) return 'unfocused';
              if (uneditable()) return 'readonly';
              if (${!!clear} && el instanceof HTMLInputElement && /^(date|datetime-local|month|time|week)$/.test(el.type)) {
                const replacement = ${JSON.stringify(input)};
                const candidate = document.createElement('input');
                candidate.type = el.type;
                candidate.value = replacement;
                if (replacement && !candidate.value) return 'invalid';
                if (el.value !== candidate.value) {
                  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(el, candidate.value);
                  el.dispatchEvent(new Event('input', { bubbles: true }));
                  el.dispatchEvent(new Event('change', { bubbles: true }));
                }
                return 'replaced';
              }
              if (${!!clear}) {
                if (el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement) el.select();
                else if (el.isContentEditable) {
                  const range = document.createRange();
                  range.selectNodeContents(el);
                  const selection = window.getSelection();
                  selection.removeAllRanges();
                  selection.addRange(range);
                } else return 'cannot-clear';
              }
              return document.activeElement === el ? 'focused' : 'unfocused';
          `);
          if (result === 'readonly') return `The field matching ${selector} is read-only or disabled`;
          if (result === 'unfocused') return `The field matching ${selector} could not hold focus`;
          if (result === 'invalid') return `The field matching ${selector} rejected the replacement value`;
          if (result === 'cannot-clear') return `The element matching ${selector} does not support text replacement`;
          replaced = result === 'replaced';
        }
        // Select for replacement first so a tab switch before insertion preserves the original text.
        if (!replaced) {
          if (element && !await element('return el.isConnected && document.activeElement === el;')) {
            return `The field matching ${selector} could not hold focus`;
          }
          const send = await debuggerFor(t);
          if (selector && clear && input === '') await dispatchKey(send, 'Backspace');
          else await send('Input.insertText', { text: input });
        }
        if (submit) {
          if (element && !await element('return el.isConnected && document.activeElement === el;')) {
            return `The field matching ${selector} lost focus before Enter`;
          }
          await dispatchKey(await debuggerFor(t), 'Enter');
        }
      };
      const problem = await withInput(t, async () => {
        if (!selector) return typeInto();
        const result = await withElement(t, selector, typeInto);
        return result === null ? `No element matches ${selector}` : result;
      });
      if (problem) return fail(problem);
      await settle(t.chromeId);
      return text('ok');
    },
    press_key: async ({ tabId, key, allowVisibleTab }, requireCurrent) => {
      const t = await editTarget(tabId, `pressing ${key}`, allowVisibleTab, requireCurrent);
      await withInput(t, async () => dispatchKey(await debuggerFor(t), String(key)));
      await settle(t.chromeId);
      return text('ok');
    },
    evaluate: async ({ tabId, expression, allowVisibleTab }, requireCurrent) => {
      const t = await editTarget(tabId, 'running JavaScript', allowVisibleTab, requireCurrent);
      const result = await withMutation(t, () => evaluate(t, expression));
      return text(result === undefined ? 'undefined' : result);
    },
    wait_for: async ({ tabId, selector, timeoutMs = 10000 }, requireCurrent) => {
      const t = await target(tabId, `waiting for ${selector ?? 'the page'}`, requireCurrent);
      const deadline = Date.now() + timeoutMs;
      while (Date.now() < deadline) {
        requireCurrent();
        const ready = selector
          ? await evaluate(t, `!!document.querySelector(${JSON.stringify(selector)})`).catch(() => false)
          : (await chrome.tabs.get(t.chromeId)).status !== 'loading';
        requireCurrent();
        if (ready) return text('ready');
        await wait(200);
      }
      return fail(`Timed out after ${timeoutMs}ms`);
    },
  };

  // ---- chrome.debugger --------------------------------------------------------
  // Attached on first use and kept while the agent works, so console messages keep coming
  // (Runtime.enable replays the ones logged before); let go after two quiet minutes.
  // Tab sleep leaves an attached page alone.

  const DEBUG_IDLE = 120_000;
  const attached = new Map<number, ReturnType<typeof setTimeout>>();
  const attaching = new Map<number, Promise<void>>();
  const waking = new Map<number, Promise<void>>();
  const consoleLogs = new Map<number, Array<{ level: string; message: string; source: string; time: number }>>();

  function keepAttached(chromeId: number) {
    clearTimeout(attached.get(chromeId));
    attached.set(
      chromeId,
      setTimeout(() => void chrome.debugger.detach({ tabId: chromeId }).catch(() => undefined).finally(() => forget(chromeId)), DEBUG_IDLE),
    );
  }

  function forget(chromeId: number) {
    clearTimeout(attached.get(chromeId));
    const was = attached.delete(chromeId);
    consoleLogs.delete(chromeId);
    if (was) void ctx.agentTabsChanged();
  }

  /**
   * Chromium keeps one extension's debugger out of a tab that shows another extension's page,
   * such as the frames a password manager adds to sign-in forms. Agent tabs keep other
   * extensions out (patch 0080), but your own tabs don't: this names the extension in the way.
   */
  async function attachRefused(chromeId: number, error: unknown): Promise<unknown> {
    // Only Chromium's refusals and detaches: other errors say what went wrong themselves.
    const message = error instanceof Error ? error.message : String(error);
    if (!/Cannot access a chrome-extension:\/\/ URL of different extension|Cannot attach to this target|Detached while handling command/.test(message)) {
      return error;
    }
    // webNavigation leaves other extensions' frames out, so they are read from the page: the
    // frame elements, in closed shadow roots too, where password managers put theirs.
    const results = await chrome.scripting
      .executeScript({
        target: { tabId: chromeId, allFrames: true },
        func: () => {
          const sources: string[] = [];
          const walk = (root: ParentNode) => {
            for (const element of root.querySelectorAll('*')) {
              if (element instanceof HTMLIFrameElement || element instanceof HTMLFrameElement) sources.push(element.src);
              const shadow = element instanceof HTMLElement ? chrome.dom.openOrClosedShadowRoot(element) : null;
              if (shadow) walk(shadow);
            }
          };
          walk(document);
          return sources;
        },
      })
      .catch(() => []);
    const others = new Set<string>();
    for (const url of results.flatMap((frame) => (frame.result as string[] | undefined) ?? [])) {
      const id = /^chrome-extension:\/\/([a-p]{32})\//.exec(url)?.[1];
      if (id && id !== chrome.runtime.id) others.add(id);
    }
    if (!others.size) return error;
    const tab = await chrome.tabs.get(chromeId).catch(() => undefined);
    const actions =
      tab && typeof chrome.flechaPrivate?.getExtensionActions === 'function'
        ? await chrome.flechaPrivate.getExtensionActions(tab.windowId).catch(() => [])
        : [];
    const names = [...others].map((id) => actions.find((action) => action.id === id)?.name ?? id);
    return new Error(
      `This tab shows a page of another extension (${names.join(', ')}), and Chromium doesn't let Flecha drive a tab with another extension's page in it. ` +
        'Open the page with open_tab instead: other extensions stay out of the tabs Flecha opens for you.',
    );
  }

  function attach(chromeId: number): Promise<void> {
    // Attached counts from the debugger's attach, but commands wait for debugger setup.
    let pending = attaching.get(chromeId);
    if (pending) return pending;
    if (attached.has(chromeId)) {
      keepAttached(chromeId);
      return Promise.resolve();
    }
    pending = (async () => {
      await chrome.debugger.attach({ tabId: chromeId }, '1.3');
      keepAttached(chromeId);
      consoleLogs.set(chromeId, []);
      try {
        // A frozen tab answers nothing; tab sleep wakes it now that it's in use, before any command.
        await ctx.agentTabsChanged();
        await chrome.debugger.sendCommand({ tabId: chromeId }, 'Runtime.enable');
        await chrome.debugger.sendCommand({ tabId: chromeId }, 'Log.enable');
        // The replayed messages arrive right after enabling.
        await wait(100);
      } catch (error) {
        // A tab left half set up would skip the setup on later calls; let go so the next call
        // starts over. onDetach doesn't fire for our own detach, so forget it here.
        await chrome.debugger.detach({ tabId: chromeId }).catch(() => undefined);
        forget(chromeId);
        throw error;
      }
    })()
      .catch(async (error: unknown) => {
        throw await attachRefused(chromeId, error);
      })
      .finally(() => attaching.delete(chromeId));
    attaching.set(chromeId, pending);
    return pending;
  }

  function sendDebug(chromeId: number, method: string, params?: object) {
    return chrome.debugger.sendCommand({ tabId: chromeId }, method, params).catch(async (error: unknown) => {
      throw await attachRefused(chromeId, error);
    });
  }

  async function attachTarget({ chromeId, checkEditable, requireCurrent }: PageTarget) {
    requireCurrent();
    if (!attached.has(chromeId) && !attaching.has(chromeId)) {
      let wake = waking.get(chromeId);
      if (!wake) {
        const state = checkEditable ? await checkEditable() : await chrome.tabs.get(chromeId);
        requireCurrent();
        wake = waking.get(chromeId);
        if (!wake && state.discarded && !attached.has(chromeId) && !attaching.has(chromeId)) {
          // Share the reload, but check each request's deadline again before attaching.
          wake = chrome.tabs.reload(chromeId).then(() => settle(chromeId)).finally(() => waking.delete(chromeId));
          waking.set(chromeId, wake);
        }
      }
      if (wake) await wake;
    }
    requireCurrent();
    await attach(chromeId);
    requireCurrent();
  }

  type SendDebug = (method: string, params?: object) => ReturnType<typeof sendDebug>;

  async function debuggerFor(tab: PageTarget): Promise<SendDebug> {
    // Attach is shared. Permission to edit belongs to each tool request.
    await attachTarget(tab);
    await tab.checkEditable?.();
    return (method, params) => sendDebug(tab.chromeId, method, params);
  }

  const mutationTail = new Map<number, Promise<void>>();

  async function withMutation<Result>(tab: PageTarget, action: () => Promise<Result>): Promise<Result> {
    const { chromeId } = tab;
    // Input, navigation and scripts share the tab's document, focus and selection.
    const run = (mutationTail.get(chromeId) ?? Promise.resolve()).then(() => {
      tab.requireCurrent();
      return action();
    });
    const tail = run.then(() => undefined, () => undefined);
    mutationTail.set(chromeId, tail);
    try {
      return await run;
    } finally {
      if (mutationTail.get(chromeId) === tail) mutationTail.delete(chromeId);
    }
  }

  async function withInput<Result>(tab: PageTarget, action: () => Promise<Result>): Promise<Result> {
    const { chromeId, checkEditable } = tab;
    return withMutation(tab, async () => {
      await attachTarget(tab);
      const emulateFocus = (await checkEditable?.())?.visible === false;
      try {
        if (emulateFocus) await sendDebug(chromeId, 'Emulation.setFocusEmulationEnabled', { enabled: true });
        return await action();
      } finally {
        if (emulateFocus) {
          try {
            await sendDebug(chromeId, 'Emulation.setFocusEmulationEnabled', { enabled: false });
          } catch (error) {
            try {
              await chrome.debugger.detach({ tabId: chromeId });
            } catch (detachError) {
              throw new Error(`Could not clear focus emulation: ${String(error)}; detaching failed: ${String(detachError)}`);
            }
            forget(chromeId);
          }
        }
      }
    });
  }

  async function debug(tab: PageTarget, method: string, params?: object) {
    const send = await debuggerFor(tab);
    return send(method, params);
  }

  async function evaluate(tab: PageTarget, expression: string): Promise<unknown> {
    return evaluateWith(await debuggerFor(tab), expression);
  }

  async function evaluateWith(send: SendDebug, expression: string): Promise<unknown> {
    const result = await send('Runtime.evaluate', {
      expression,
      awaitPromise: true,
      returnByValue: true,
      userGesture: true,
    });
    return evaluationResult(result)?.value;
  }

  function evaluationResult(response: unknown) {
    const result = response as { result?: { value?: unknown; objectId?: string }; exceptionDetails?: { text?: string; exception?: { description?: string } } };
    if (result.exceptionDetails) {
      throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text ?? 'The script threw');
    }
    return result.result;
  }

  type ElementCall = (body: string) => Promise<unknown>;

  async function withElement<Result>(tab: PageTarget, selector: string, action: (call: ElementCall) => Promise<Result>): Promise<Result | null> {
    const objectId = evaluationResult(await debug(tab, 'Runtime.evaluate', {
      expression: `document.querySelector(${JSON.stringify(selector)})`,
      returnByValue: false,
    }))?.objectId;
    if (!objectId) return null;
    const call: ElementCall = async (body) => evaluationResult(await debug(tab, 'Runtime.callFunctionOn', {
      objectId, functionDeclaration: `function() { const el = this; ${body} }`, returnByValue: true, userGesture: true,
    }))?.value;
    try {
      return await action(call);
    } finally {
      await sendDebug(tab.chromeId, 'Runtime.releaseObject', { objectId }).catch(() => undefined);
    }
  }

  chrome.debugger.onEvent.addListener((source, method, params: any) => {
    const log = source.tabId !== undefined ? consoleLogs.get(source.tabId) : undefined;
    if (!log) return;
    if (method === 'Runtime.consoleAPICalled') {
      const level = { warning: 'warning', error: 'error', assert: 'error', debug: 'debug' }[params.type as string] ?? 'info';
      const message = (params.args ?? []).map((a: any) => (a.value !== undefined ? String(a.value) : (a.description ?? a.type))).join(' ');
      const frame = params.stackTrace?.callFrames?.[0];
      log.push({ level, message, source: frame ? `${frame.url}:${frame.lineNumber + 1}` : '', time: params.timestamp ?? Date.now() });
    } else if (method === 'Runtime.exceptionThrown') {
      const d = params.exceptionDetails ?? {};
      log.push({ level: 'error', message: d.exception?.description ?? d.text ?? 'Uncaught exception', source: d.url ? `${d.url}:${(d.lineNumber ?? 0) + 1}` : '', time: params.timestamp ?? Date.now() });
    } else if (method === 'Log.entryAdded') {
      const e = params.entry ?? {};
      const level = e.level === 'verbose' ? 'debug' : e.level === 'warning' || e.level === 'error' ? e.level : 'info';
      log.push({ level, message: e.text ?? '', source: e.url ? `${e.url}${e.lineNumber !== undefined ? `:${e.lineNumber + 1}` : ''}` : '', time: e.timestamp ?? Date.now() });
    } else if (method === 'Runtime.executionContextsCleared') {
      // A new page; Electron's log kept going across navigations too, so leave it.
    }
    if (log.length > 500) log.splice(0, log.length - 500);
  });
  chrome.debugger.onDetach.addListener((source) => source.tabId !== undefined && forget(source.tabId));
  chrome.tabs.onRemoved.addListener((chromeId) => forget(chromeId));

  async function settleGesture(commands: Promise<unknown>[]) {
    const outcomes = await Promise.allSettled(commands);
    const failed = outcomes.find((outcome) => outcome.status === 'rejected');
    if (failed) throw failed.reason;
  }

  async function dispatchKey(send: SendDebug, key: string) {
    const named: Record<string, [code: string, keyCode: number, text?: string]> = {
      Enter: ['Enter', 13, '\r'],
      Tab: ['Tab', 9],
      Escape: ['Escape', 27],
      Backspace: ['Backspace', 8],
      Delete: ['Delete', 46],
      ArrowLeft: ['ArrowLeft', 37],
      ArrowUp: ['ArrowUp', 38],
      ArrowRight: ['ArrowRight', 39],
      ArrowDown: ['ArrowDown', 40],
      Home: ['Home', 36],
      End: ['End', 35],
      PageUp: ['PageUp', 33],
      PageDown: ['PageDown', 34],
      ' ': ['Space', 32, ' '],
    };
    const [code, keyCode, keyText] =
      named[key] ?? (key.length === 1 ? [`Key${key.toUpperCase()}`, key.toUpperCase().charCodeAt(0), key] : [key, 0]);
    const base = { key, code, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode };
    // Send the gesture as a pair after one check so a tab switch cannot strand a key down.
    await settleGesture([
      send('Input.dispatchKeyEvent', { type: keyText ? 'keyDown' : 'rawKeyDown', ...base, text: keyText, unmodifiedText: keyText }),
      send('Input.dispatchKeyEvent', { type: 'keyUp', ...base }),
    ]);
  }

  return {
    /** For UiState.agent: what a coding agent is doing, for the sidebar. */
    agentActivity(): UiState['agent'] {
      return (
        activity && {
          connected: Date.now() - activity.at < 60_000,
          busy: Date.now() - activity.at < 3_500,
          tabId: activity.tabId,
          action: activity.action,
        }
      );
    },
    /** The Chromium tabs the agent is working in (debugger attached), which tab sleep treats as in use. */
    agentTabsInUse: (): Set<number> => new Set(attached.keys()),
    /** The bridge's address while it runs, for Settings. */
    agentEndpoint: () => endpoint,
    /** Why the host or its bridge last failed, for Settings; undefined once they work. */
    hostError: () => hostError ?? agentError,
  };
}

const wait = (ms: number) => new Promise((r) => setTimeout(r, ms));

const text = (value: unknown): AgentResult => ({
  content: [{ type: 'text', text: typeof value === 'string' ? value : JSON.stringify(value, null, 2) }],
});
const fail = (message: string): AgentResult => ({ content: [{ type: 'text', text: message }], isError: true });

/** Waits for navigation the last action may have started, up to a few seconds. */
async function settle(chromeId: number) {
  await wait(150);
  const deadline = Date.now() + 8000;
  while (Date.now() < deadline) {
    const tab = await chrome.tabs.get(chromeId).catch(() => undefined);
    if (!tab || tab.status !== 'loading') return;
    await wait(100);
  }
}

/** Scales a base64 PNG down to `width` pixels wide, as Electron's bridge did. */
async function shrink(png: string, width: number): Promise<string> {
  const bytes = Uint8Array.from(atob(png), (c) => c.charCodeAt(0));
  const image = await createImageBitmap(new Blob([bytes], { type: 'image/png' }));
  if (image.width <= width) return png;
  const height = Math.round((image.height * width) / image.width);
  const canvas = new OffscreenCanvas(width, height);
  canvas.getContext('2d')!.drawImage(image, 0, 0, width, height);
  const out = new Uint8Array(await (await canvas.convertToBlob({ type: 'image/png' })).arrayBuffer());
  let binary = '';
  for (let i = 0; i < out.length; i += 0x8000) binary += String.fromCharCode(...out.subarray(i, i + 0x8000));
  return btoa(binary);
}
