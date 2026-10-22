import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { execFile } from 'node:child_process';
import { promisify } from 'node:util';
import { app } from 'electron';
import type { HistoryEntry } from '../shared/types';
import type { History } from './history';
import type { Store } from './store';
import type { ImportNode } from './arcImport';

/**
 * Brings bookmarks and history over from the other browsers on this Mac. Their files are
 * only ever read: databases are copied to a temporary folder first, since the browser may
 * have them open and locked. Passwords and cookies are left where they are.
 */

const run = promisify(execFile);

/** Pages taken from each profile's history, newest first. Flecha keeps 5,000 in all. */
const HISTORY_ROWS = 5000;
/** Bookmarks past this many are left behind: no sidebar can usefully hold more. */
const MAX_BOOKMARKS = 2000;
/** Folders nested deeper than this are flattened into their parent. */
const MAX_DEPTH = 8;

export interface DetectedBrowser {
  id: string;
  name: string;
  profiles: number;
  hasBookmarks: boolean;
  hasHistory: boolean;
}

export interface ImportResult {
  spaces: number;
  bookmarks: number;
  history: number;
  warnings: string[];
}

/** One profile of a browser, with whichever of its files exist. */
interface Profile {
  name: string;
  bookmarkFiles: string[];
  historyFile?: string;
}

interface BrowserSource {
  id: string;
  name: string;
  profiles(): Profile[];
  readBookmarks(profile: Profile): Promise<ImportNode[]>;
  readHistory(profile: Profile): Promise<HistoryEntry[]>;
}

const appSupport = (...parts: string[]) => path.join(app.getPath('appData'), ...parts);

// ---- Chromium family -------------------------------------------------------

const CHROMIUM: Array<[id: string, name: string, dir: string]> = [
  ['chrome', 'Chrome', 'Google/Chrome'],
  ['brave', 'Brave', 'BraveSoftware/Brave-Browser'],
  ['edge', 'Edge', 'Microsoft Edge'],
  ['vivaldi', 'Vivaldi', 'Vivaldi'],
  ['opera', 'Opera', 'com.operasoftware.Opera'],
  ['opera-gx', 'Opera GX', 'com.operasoftware.OperaGX'],
  ['chromium', 'Chromium', 'Chromium'],
];

function chromiumSource(id: string, name: string, root: string): BrowserSource {
  return {
    id,
    name,
    profiles: () => chromiumProfiles(root),
    readBookmarks: async (profile) => chromiumBookmarks(profile.bookmarkFiles),
    readHistory: (profile) =>
      // Chromium counts in microseconds since 1601.
      querySqlite<HistoryEntry>(
        profile.historyFile!,
        `SELECT url, title, visit_count AS visits, last_visit_time / 1000 - 11644473600000 AS lastVisit
         FROM urls WHERE hidden = 0 AND last_visit_time > 0 ORDER BY last_visit_time DESC LIMIT ${HISTORY_ROWS}`,
      ),
  };
}

/** `Default` and `Profile N` folders (Opera keeps its one profile in the root itself). */
function chromiumProfiles(root: string): Profile[] {
  const names = chromiumProfileNames(root);
  const dirs = [root, ...listDirs(root).filter((d) => /^(Default|Profile \d+)$/.test(d)).map((d) => path.join(root, d))];
  return dirs.flatMap((dir) => {
    const key = dir === root ? 'Default' : path.basename(dir);
    // Chrome keeps the bookmarks of a signed-in account in a file of their own.
    const bookmarkFiles = ['Bookmarks', 'AccountBookmarks'].map((f) => path.join(dir, f)).filter(isFile);
    const history = path.join(dir, 'History');
    const profile: Profile = { name: names[key] || key, bookmarkFiles, historyFile: isFile(history) ? history : undefined };
    return profile.bookmarkFiles.length || profile.historyFile ? [profile] : [];
  });
}

/** The names people gave their profiles, from `Local State`. */
function chromiumProfileNames(root: string): Record<string, string> {
  try {
    const cache = JSON.parse(fs.readFileSync(path.join(root, 'Local State'), 'utf8'))?.profile?.info_cache ?? {};
    return Object.fromEntries(Object.entries(cache).map(([dir, info]: [string, any]) => [dir, String(info?.name ?? '')]));
  } catch {
    return {};
  }
}

/** The bookmarks bar becomes the top level; "Other" and "Mobile" bookmarks get folders of their own. */
function chromiumBookmarks(files: string[]): ImportNode[] {
  const bar: ImportNode[] = [];
  const other: ImportNode[] = [];
  const mobile: ImportNode[] = [];
  for (const file of files) {
    const roots = JSON.parse(fs.readFileSync(file, 'utf8'))?.roots ?? {};
    bar.push(...chromiumChildren(roots.bookmark_bar));
    other.push(...chromiumChildren(roots.other));
    mobile.push(...chromiumChildren(roots.synced));
  }
  return [...bar, folder('Other Bookmarks', other), folder('Mobile Bookmarks', mobile)];
}

function chromiumChildren(node: any): ImportNode[] {
  return Array.isArray(node?.children) ? node.children.flatMap(chromiumNode) : [];
}

function chromiumNode(node: any): ImportNode[] {
  if (node?.type === 'url' && typeof node.url === 'string') return [{ kind: 'tab', url: node.url, title: String(node.name || node.url) }];
  if (node?.type === 'folder') return [folder(String(node.name || 'Untitled folder'), chromiumChildren(node))];
  return [];
}

// ---- detecting and importing -------------------------------------------------

function sources(): BrowserSource[] {
  return [...CHROMIUM.map(([id, name, dir]) => chromiumSource(id, name, appSupport(dir)))];
}

/** Browsers on this Mac that have bookmarks or history to bring over. Only checks which files exist. */
export async function detectBrowsers(): Promise<DetectedBrowser[]> {
  return sources().flatMap((source) => {
    try {
      const profiles = source.profiles();
      if (!profiles.length) return [];
      return [
        {
          id: source.id,
          name: source.name,
          profiles: profiles.length,
          hasBookmarks: profiles.some((p) => p.bookmarkFiles.length > 0),
          hasHistory: profiles.some((p) => !!p.historyFile),
        },
      ];
    } catch {
      return [];
    }
  });
}

/**
 * Bookmarks become pinned tabs in a new Space named after the browser (with a folder per
 * profile when several have some), and history is merged into Flecha's. Problems with one
 * profile or file become warnings rather than failing the whole import.
 */
export async function importBrowser(id: string, store: Store, history: History): Promise<ImportResult> {
  const result: ImportResult = { spaces: 0, bookmarks: 0, history: 0, warnings: [] };
  const source = sources().find((s) => s.id === id);
  if (!source) {
    result.warnings.push(`No data from “${id}” was found on this Mac.`);
    return result;
  }

  let profiles: Profile[] = [];
  try {
    profiles = source.profiles();
  } catch (err) {
    result.warnings.push(`Couldn’t look for ${source.name} profiles: ${reason(err)}`);
  }
  const label = (p: Profile) => (profiles.length > 1 ? `${source.name} (${p.name})` : source.name);

  const tidy = newTidy();
  const perProfile: Array<{ profile: Profile; nodes: ImportNode[] }> = [];
  const visits: HistoryEntry[] = [];
  for (const profile of profiles) {
    if (profile.bookmarkFiles.length) {
      try {
        const nodes = tidy.run(await source.readBookmarks(profile));
        if (nodes.length) perProfile.push({ profile, nodes });
      } catch (err) {
        result.warnings.push(`Couldn’t read bookmarks from ${label(profile)}: ${reason(err)}`);
      }
    }
    if (profile.historyFile) {
      try {
        visits.push(...(await source.readHistory(profile)).map(cleanEntry));
      } catch (err) {
        result.warnings.push(`Couldn’t read history from ${label(profile)}: ${reason(err)}`);
      }
    }
  }

  if (tidy.skipped) result.warnings.push(`Only the first ${MAX_BOOKMARKS} bookmarks were imported; ${tidy.skipped} more were left out.`);
  if (perProfile.length) {
    const nodes = perProfile.length > 1 ? perProfile.map(({ profile, nodes }) => folder(profile.name, nodes)) : perProfile[0].nodes;
    store.importBookmarks(source.name, nodes);
    result.spaces = 1;
    result.bookmarks = tidy.count;
  }
  if (visits.length) result.history = history.merge(visits);
  return result;
}

// ---- helpers ---------------------------------------------------------------

/**
 * Keeps web bookmarks only, each URL once, drops empty folders and stops at
 * MAX_BOOKMARKS. Shared across a browser's profiles so the cap covers them all.
 */
function newTidy() {
  const seen = new Set<string>();
  const tidy = {
    skipped: 0,
    get count() {
      return seen.size;
    },
    run(nodes: ImportNode[], depth = 0): ImportNode[] {
      return nodes.flatMap((n): ImportNode[] => {
        if (n.kind === 'tab') {
          if (!/^https?:/i.test(n.url) || seen.has(n.url)) return [];
          if (seen.size >= MAX_BOOKMARKS) {
            tidy.skipped++;
            return [];
          }
          seen.add(n.url);
          return [n];
        }
        if (depth >= MAX_DEPTH) return tidy.run(n.children, depth);
        const children = tidy.run(n.children, depth + 1);
        return children.length ? [{ ...n, children }] : [];
      });
    },
  };
  return tidy;
}

function folder(name: string, children: ImportNode[]): ImportNode {
  return { kind: 'folder', name, children };
}

/** Databases hand back nulls and odd types; History only wants plain values. */
function cleanEntry(row: HistoryEntry): HistoryEntry {
  const url = String(row.url ?? '');
  return { url, title: String(row.title || url), visits: Number(row.visits) || 1, lastVisit: Number(row.lastVisit) };
}

/** Runs `sql` against a copy of `file`, since the browser may have it open and locked. */
async function querySqlite<T>(file: string, sql: string): Promise<T[]> {
  const dir = await fs.promises.mkdtemp(path.join(os.tmpdir(), 'flecha-import-'));
  try {
    const copy = path.join(dir, path.basename(file));
    await fs.promises.copyFile(file, copy);
    // Recent writes may still be in the write-ahead log rather than the database itself.
    await fs.promises.copyFile(`${file}-wal`, `${copy}-wal`).catch(() => {});
    return await runSql<T>(copy, sql);
  } finally {
    await fs.promises.rm(dir, { recursive: true, force: true });
  }
}

/** Node's built-in SQLite when this runtime has it, macOS's sqlite3 tool otherwise. */
async function runSql<T>(file: string, sql: string): Promise<T[]> {
  let sqlite: typeof import('node:sqlite') | undefined;
  try {
    sqlite = require('node:sqlite');
  } catch {
    // Not built in; fall back to the command line tool below.
  }
  if (sqlite) {
    const db = new sqlite.DatabaseSync(file);
    try {
      return db.prepare(sql).all() as T[];
    } finally {
      db.close();
    }
  }
  const { stdout } = await run('/usr/bin/sqlite3', ['-json', file, sql], { maxBuffer: 64 * 1024 * 1024 });
  return stdout.trim() ? (JSON.parse(stdout) as T[]) : [];
}

function listDirs(dir: string): string[] {
  try {
    return fs.readdirSync(dir, { withFileTypes: true }).filter((d) => d.isDirectory()).map((d) => d.name);
  } catch {
    return [];
  }
}

function isFile(file: string) {
  try {
    return fs.statSync(file).isFile();
  } catch {
    return false;
  }
}

function reason(err: unknown) {
  return err instanceof Error ? err.message : String(err);
}
