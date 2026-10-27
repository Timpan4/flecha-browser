import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import type { ArchivedTab } from '../shared/types';
import type { ArcImport, ArcProfile, ArcSpace, ImportNode, ImportTab } from '../shared/importApply';

/**
 * Reads Arc's local files: StorableSidebar.json in ~/Library/Application Support/Arc on macOS,
 * and on Windows in Arc's package folder (browserImport.ts arcWindowsDir), with
 * StorableArchiveItems.json and `User Data/Local State` beside it.
 * docs/arc-import-format.md describes the format and which importers it was confirmed against.
 *
 * Shape, as those importers read it: `sidebar.containers[n]` holds `spaces` and `items`, both
 * flat arrays alternating `"<id>", { ...object }`. Each space lists
 * `containerIDs: ["pinned", <id>, "unpinned", <id>]`, which point at items whose
 * `childrenIds` are the sidebar entries. An item's `data` is `{ tab: { savedURL, savedTitle } }`
 * for a tab, `{ list: {} }` for a folder or `{ splitView: {} }` for a split, whose children are
 * its tabs. `topAppsContainerIDs` alternates a profile and the favorites container of that profile.
 */

export type { ArcImport, ImportNode, ImportTab } from '../shared/importApply';

interface ArcItem {
  id: string;
  title?: string | null;
  childrenIds?: string[];
  data?: Record<string, any>;
}

interface ArcRawSpace {
  id: string;
  title?: string | null;
  containerIDs?: unknown[];
  newContainerIDs?: unknown[];
  profile?: { default?: boolean; custom?: { _0?: { directoryBasename?: string } } };
  customInfo?: {
    iconType?: { emoji_v2?: string };
    windowTheme?: { primaryColorPalette?: { midTone?: { red?: number; green?: number; blue?: number } } };
  };
}

/** Seconds between 1970 and 2001, where Apple's reference dates start. */
const APPLE_EPOCH = 978307200;

/** Electron's appData on macOS; spelled out so the native messaging host can read it without Electron. */
export function arcDir() {
  return path.join(os.homedir(), 'Library', 'Application Support', 'Arc');
}

/** Arc's sidebar, archive and profile names from its folder. Only the sidebar has to be there. */
export function readArc(dir = arcDir()): { arc: ArcImport; warnings: string[] } {
  const warnings: string[] = [];
  const optional = (file: string, what: string) => {
    const full = path.join(dir, file);
    if (!fs.existsSync(full)) return undefined;
    try {
      return JSON.parse(fs.readFileSync(full, 'utf8'));
    } catch (err) {
      warnings.push(`Couldn’t read Arc’s ${what}: ${err instanceof Error ? err.message : String(err)}`);
      return undefined;
    }
  };
  const names = profileNames(optional(path.join('User Data', 'Local State'), 'profile names'));
  const arc = parseArcSidebar(JSON.parse(fs.readFileSync(path.join(dir, 'StorableSidebar.json'), 'utf8')), names);
  const archive = optional('StorableArchiveItems.json', 'archived tabs');
  if (archive) arc.archive = parseArcArchive(archive);
  return { arc, warnings };
}

/** Profile folder → the name people gave it, from Chromium's `Local State`. */
function profileNames(localState: any): Record<string, string> {
  const cache = localState?.profile?.info_cache;
  if (!isObject(cache)) return {};
  return Object.fromEntries(Object.entries(cache).flatMap(([dir, info]) => (typeof info?.name === 'string' ? [[dir, info.name]] : [])));
}

export function parseArcSidebar(json: any, names: Record<string, string> = {}): ArcImport {
  const containers: any[] = json?.sidebar?.containers ?? [];
  const main = containers.find((c) => Array.isArray(c?.spaces) && Array.isArray(c?.items));
  if (!main) throw new Error('This does not look like an Arc StorableSidebar.json file.');

  const items = new Map<string, ArcItem>();
  for (const entry of main.items) if (isObject(entry) && typeof entry.id === 'string') items.set(entry.id, entry as ArcItem);
  const skipped: Record<string, number> = {};
  const skip = (what: string) => (skipped[what] = (skipped[what] ?? 0) + 1);

  const toTab = (item: ArcItem): ImportTab | undefined => {
    const url = item.data?.tab?.savedURL;
    if (typeof url !== 'string' || !url) return undefined;
    if (!/^https?:/i.test(url)) return void skip('non-web tab');
    return { url, title: item.title || item.data?.tab?.savedTitle || url };
  };

  /** Splits found while walking a Space, as the addresses of their tabs. */
  let splits: string[][] = [];
  const toNodes = (ids: unknown = [], seen = new Set<string>()): ImportNode[] =>
    (Array.isArray(ids) ? ids : []).flatMap((id): ImportNode[] => {
      const item = typeof id === 'string' ? items.get(id) : undefined;
      // An item listed twice, or a loop, is taken once.
      if (!item || seen.has(item.id)) return [];
      seen.add(item.id);
      const data = item.data ?? {};
      if (data.list) return [{ kind: 'folder', name: item.title || 'Untitled folder', children: toNodes(item.childrenIds, seen) }];
      if (data.tab) {
        const tab = toTab(item);
        // Arc can keep tabs under a tab; Flecha can't, so they follow it.
        return [...(tab ? [{ kind: 'tab' as const, ...tab }] : []), ...toNodes(item.childrenIds, seen)];
      }
      const children = toNodes(item.childrenIds, seen);
      if (data.splitView) {
        const urls = children.flatMap((n) => (n.kind === 'tab' ? [n.url] : []));
        if (urls.length > 1) splits.push(urls);
      } else {
        skip(Object.keys(data)[0] ?? 'unknown');
      }
      return children;
    });

  const flatTabs = (nodes: ImportNode[]): ImportTab[] =>
    nodes.flatMap((n) => (n.kind === 'tab' ? [{ url: n.url, title: n.title }] : flatTabs(n.children)));

  // Markers are "pinned"/"unpinned", or objects keyed by them; some files call the list newContainerIDs.
  const containerChildren = (space: ArcRawSpace, key: 'pinned' | 'unpinned') => {
    const ids = space.containerIDs ?? space.newContainerIDs ?? [];
    const at = ids.findIndex((m) => m === key || (isObject(m) && key in m));
    const id = at >= 0 ? ids[at + 1] : undefined;
    return typeof id === 'string' ? items.get(id)?.childrenIds : undefined;
  };

  const spaces = (main.spaces as unknown[]).filter(isObject).map((raw): ArcSpace => {
    const space = raw as ArcRawSpace;
    splits = [];
    const pinned = toNodes(containerChildren(space, 'pinned'));
    const today = flatTabs(toNodes(containerChildren(space, 'unpinned')));
    return {
      name: space.title || 'Space',
      emoji: space.customInfo?.iconType?.emoji_v2,
      color: themeColor(space.customInfo?.windowTheme?.primaryColorPalette?.midTone),
      profile: profileOf(space.profile, names),
      pinned,
      today,
      splits,
    };
  });

  // One favorites container per profile, after its profile; Flecha's favorites are shared, so all of them.
  const favorites: ImportTab[] = [];
  splits = [];
  for (const id of (main.topAppsContainerIDs as unknown[] | undefined) ?? []) {
    if (typeof id !== 'string' || !items.has(id)) continue;
    for (const tab of flatTabs(toNodes(items.get(id)?.childrenIds))) if (!favorites.some((f) => f.url === tab.url)) favorites.push(tab);
  }

  return { spaces, favorites, archive: [], skipped };
}

/**
 * StorableArchiveItems.json: `items` alternates an id and `{ archivedAt, reason, sidebarItem }`,
 * `archivedAt` in seconds since 2001 and `sidebarItem` shaped like a sidebar tab. Newest first,
 * each address once (its latest archiving).
 */
export function parseArcArchive(json: any): ArchivedTab[] {
  const latest = new Map<string, ArchivedTab>();
  for (const entry of Array.isArray(json?.items) ? json.items : []) {
    if (!isObject(entry) || typeof entry.archivedAt !== 'number') continue;
    const item = entry.sidebarItem;
    const url = item?.data?.tab?.savedURL;
    if (typeof url !== 'string' || !/^https?:/i.test(url)) continue;
    const tab = { url, title: String(item.title || item.data.tab.savedTitle || url), closedAt: Math.round((entry.archivedAt + APPLE_EPOCH) * 1000) };
    if ((latest.get(url)?.closedAt ?? -Infinity) < tab.closedAt) latest.set(url, tab);
  }
  return [...latest.values()].sort((a, b) => b.closedAt - a.closedAt);
}

/** Arc's colours are extended sRGB, 0–1 per channel but able to go past either end. */
function themeColor(c: { red?: number; green?: number; blue?: number } | undefined): string | undefined {
  if (!c || ![c.red, c.green, c.blue].every((v) => typeof v === 'number' && Number.isFinite(v))) return undefined;
  return '#' + [c.red!, c.green!, c.blue!].map((v) => Math.round(Math.min(1, Math.max(0, v)) * 255).toString(16).padStart(2, '0')).join('');
}

/** `{ default: true }` is Arc's own profile; others name their folder under `User Data`. */
function profileOf(profile: ArcRawSpace['profile'], names: Record<string, string>): ArcProfile | undefined {
  const dir = profile?.custom?._0?.directoryBasename;
  return typeof dir === 'string' && dir ? { dir, name: names[dir] || dir } : undefined;
}

function isObject(v: unknown): v is Record<string, any> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}
