import type { TabInfo, WindowSpace } from './types';

/**
 * Which Today tabs the sidebar lists in the coding agent's folder rather than among Today's tabs.
 * The sidebar draws by it and the Store's bulk closes act by it, so they agree on what is where.
 */

/** The split Today draws as one row: both halves are Today tabs. */
export function todaySplit(tabs: Record<string, TabInfo>, ws: WindowSpace): [string, string] | undefined {
  return ws.split && ws.split.every((id) => ws.today.includes(id) && tabs[id]) ? ws.split : undefined;
}

/** A Today tab an agent opened, unless it is half of the split Today draws. */
export function inAgentFolder(tabs: Record<string, TabInfo>, ws: WindowSpace, tabId: string): boolean {
  return !!tabs[tabId]?.agent && !todaySplit(tabs, ws)?.includes(tabId);
}
