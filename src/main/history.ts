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
