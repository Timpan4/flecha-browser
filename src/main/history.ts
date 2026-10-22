import fs from 'node:fs';
import path from 'node:path';
import type { HistoryEntry } from '../shared/types';
import { writeFileAtomic } from './files';

const LIMIT = 5000;
const DAY = 24 * 60 * 60 * 1000;

/** Every page visited, kept in its own file so the sidebar state stays small. */
export class History {
  private entries = new Map<string, HistoryEntry>();
  private file: string;
  private saveTimer?: NodeJS.Timeout;

  constructor(dir: string) {
    this.file = path.join(dir, 'flecha-history.json');
    try {
      for (const entry of JSON.parse(fs.readFileSync(this.file, 'utf8')) as HistoryEntry[]) this.entries.set(entry.url, entry);
    } catch {
      // No history yet.
    }
  }

  record(url: string, title: string) {
    if (!/^https?:/.test(url)) return;
    const entry = this.entries.get(url);
    if (entry) {
      entry.visits++;
      entry.lastVisit = Date.now();
      if (title) entry.title = title;
    } else {
      this.entries.set(url, { url, title: title || url, visits: 1, lastVisit: Date.now() });
      if (this.entries.size > LIMIT) this.dropOldest();
    }
    this.scheduleSave();
  }

  /**
   * Folds in history from another browser. A page known to both keeps the higher visit
   * count and the later visit. Returns how many of `imported` are still kept afterwards
   * (the oldest pages fall out once the history is full).
   */
  merge(imported: HistoryEntry[]): number {
    const urls = new Set<string>();
    for (const e of imported) {
      if (!/^https?:/.test(e.url) || !Number.isFinite(e.lastVisit)) continue;
      urls.add(e.url);
      const entry = this.entries.get(e.url);
      if (entry) {
        entry.visits = Math.max(entry.visits, e.visits);
        entry.lastVisit = Math.max(entry.lastVisit, e.lastVisit);
        if (!entry.title || entry.title === entry.url) entry.title = e.title || entry.title;
      } else {
        this.entries.set(e.url, { url: e.url, title: e.title || e.url, visits: Math.max(1, e.visits), lastVisit: e.lastVisit });
      }
    }
    if (this.entries.size > LIMIT) this.dropOldest();
    this.scheduleSave();
    return [...urls].filter((url) => this.entries.has(url)).length;
  }

  updateTitle(url: string, title: string) {
    const entry = this.entries.get(url);
    if (!entry || !title || entry.title === title) return;
    entry.title = title;
    this.scheduleSave();
  }

  /** Every word must appear in the title or URL; often and recently visited pages rank first. */
  search(query: string, limit = 8): HistoryEntry[] {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return [];
    const now = Date.now();
    const score = (e: HistoryEntry) => e.visits / (1 + (now - e.lastVisit) / DAY);
    return [...this.entries.values()]
      .filter((e) => {
        const text = `${e.title} ${e.url}`.toLowerCase();
        return words.every((w) => text.includes(w));
      })
      .sort((a, b) => score(b) - score(a))
      .slice(0, limit);
  }

  /** The most visited sites (one page per host, its most visited), for the empty page. */
  top(limit = 8): HistoryEntry[] {
    const now = Date.now();
    const score = (e: HistoryEntry) => e.visits / (1 + (now - e.lastVisit) / (7 * DAY));
    const best = new Map<string, HistoryEntry>();
    for (const e of this.entries.values()) {
      let host: string;
      try {
        host = new URL(e.url).hostname.replace(/^www\./, '');
      } catch {
        continue;
      }
      // Sign-in and redirect hops are visited often but never worth going back to.
      if (/^(accounts|login|auth|signin)\./.test(host)) continue;
      const current = best.get(host);
      if (!current || score(e) > score(current)) best.set(host, e);
    }
    return [...best.values()].sort((a, b) => score(b) - score(a)).slice(0, limit);
  }

  saveNow() {
    clearTimeout(this.saveTimer);
    fs.mkdirSync(path.dirname(this.file), { recursive: true });
    writeFileAtomic(this.file, JSON.stringify([...this.entries.values()]));
  }

  private scheduleSave() {
    clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => this.saveNow(), 2000);
  }

  private dropOldest() {
    const oldest = [...this.entries.values()].sort((a, b) => a.lastVisit - b.lastVisit).slice(0, this.entries.size - LIMIT);
    for (const e of oldest) this.entries.delete(e.url);
  }
}
