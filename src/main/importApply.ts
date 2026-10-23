import type { HistoryEntry, ImportResult } from '../shared/types';
import type { ArcImport, ImportNode } from './arcImport';
import type { Store } from './store';

/**
 * What `readBrowser` (browserImport.ts) found in another browser, before any of it is added
 * to Flecha. Plain data, so it can cross from the Chromium build's native host to the shell.
 */
export interface BrowserImportData {
  /** The browser's name, which the new Space of bookmarks takes. */
  name: string;
  /** Arc's sidebar: its Spaces, pinned tabs and favourites, in place of bookmarks. */
  arc?: ArcImport;
  /** Bookmarks for one new Space, already tidied (web pages only, each once). */
  bookmarks?: ImportNode[];
  bookmarkCount: number;
  history: HistoryEntry[];
  warnings: string[];
}

/**
 * Adds what was read to Flecha: Arc's Spaces, or a Space of bookmarks, and the history.
 * Kept free of Node so the shell extension can run it too.
 */
export async function applyBrowserImport(
  data: BrowserImportData,
  store: Pick<Store, 'importArc' | 'importBookmarks'>,
  history: { merge(entries: HistoryEntry[]): number | Promise<number> },
): Promise<ImportResult> {
  const result: ImportResult = { spaces: 0, bookmarks: 0, history: 0, warnings: [...data.warnings] };
  if (data.arc) {
    store.importArc(data.arc);
    result.spaces = data.arc.spaces.length;
    result.bookmarks = data.arc.spaces.reduce((n, s) => n + countNodes(s.pinned) + s.today.length, data.arc.favorites.length);
  }
  if (data.bookmarks?.length) {
    store.importBookmarks(data.name, data.bookmarks);
    result.spaces = 1;
    result.bookmarks = data.bookmarkCount;
  }
  if (data.history.length) result.history = await history.merge(data.history);
  return result;
}

function countNodes(nodes: ImportNode[]): number {
  return nodes.reduce((n, node) => n + (node.kind === 'tab' ? 1 : countNodes(node.children)), 0);
}
