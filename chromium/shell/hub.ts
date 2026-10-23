/**
 * The background side of the shell's messaging. Every Flecha page (sidebar, Settings, …)
 * connects a port named "flecha"; commands are fire-and-forget, invokes get a reply, and
 * events go out to every connected page. Feature modules register their own handlers, so
 * they never need to touch one shared switch.
 */
import type { Message } from './protocol';

type Handler = (...args: any[]) => unknown;

const commands = new Map<string, Handler>();
const ports = new Set<chrome.runtime.Port>();

/** Adds handlers by name; a name registered twice is a bug, so it throws. */
export function handle(handlers: Record<string, Handler>) {
  for (const [name, fn] of Object.entries(handlers)) {
    if (commands.has(name)) throw new Error(`Flecha shell: "${name}" is handled twice`);
    commands.set(name, fn);
  }
}

/** Sends an event to every open Flecha page. */
export function broadcast(name: string, ...args: unknown[]) {
  const message: Message = { type: 'event', name, args };
  for (const port of ports) port.postMessage(message);
}

/** Calls fn whenever a page connects (to send it the current state, say). */
const connectListeners: Array<(port: chrome.runtime.Port) => void> = [];
export function onConnect(fn: (port: chrome.runtime.Port) => void) {
  connectListeners.push(fn);
}

export function listen() {
  chrome.runtime.onConnect.addListener((port) => {
    if (port.name !== 'flecha') return;
    ports.add(port);
    port.onDisconnect.addListener(() => ports.delete(port));
    port.onMessage.addListener(async (message: Message) => {
      if (message.type === 'ping') return;
      if (message.type !== 'cmd' && message.type !== 'invoke') return;
      const fn = commands.get(message.name);
      if (!fn) {
        console.warn(`Flecha shell: "${message.name}" isn't in the Chromium build yet`);
        if (message.type === 'invoke') port.postMessage({ type: 'result', id: message.id, value: undefined } satisfies Message);
        return;
      }
      try {
        const value = await fn(...message.args);
        if (message.type === 'invoke') port.postMessage({ type: 'result', id: message.id, value } satisfies Message);
      } catch (err) {
        console.error(`Flecha shell: "${message.name}" failed`, err);
        if (message.type === 'invoke') port.postMessage({ type: 'result', id: message.id, error: String(err) } satisfies Message);
      }
    });
    for (const fn of connectListeners) fn(port);
  });
}
