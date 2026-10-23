/**
 * Icons for sites Chromium has no icon for yet (tabs imported and never opened), fetched
 * straight from the site the way the Electron spike's src/main/favicons.ts does (no
 * third-party favicon service): the icon the home page declares, else /favicon.ico. Each
 * site is fetched once and cached in extension storage; pages get them as data URLs.
 */

import { declaredIcons } from '../../src/main/declaredIcons';

const STORAGE_KEY = 'siteIcons';
const RETRY_MISSES_AFTER = 7 * 24 * 60 * 60 * 1000;
const MAX_BYTES = 300_000;
const CONCURRENCY = 4;

type Entry = { url: string } | { missedAt: number };

export class SiteIcons {
  private entries: Record<string, Entry> = {};
  private loaded: Promise<void>;
  private queue: string[] = [];
  private active = 0;
  private saveTimer?: ReturnType<typeof setTimeout>;

  /** onFound gets the icons found since the last call, by host, as data URLs. */
  constructor(private onFound: (icons: Record<string, string>) => void) {
    this.loaded = chrome.storage.local.get(STORAGE_KEY).then((saved) => {
      this.entries = (saved[STORAGE_KEY] as Record<string, Entry> | undefined) ?? {};
    });
  }

  /** Every cached icon, by host, for a page that just connected. */
  async all(): Promise<Record<string, string>> {
    await this.loaded;
    return Object.fromEntries(Object.entries(this.entries).flatMap(([host, e]) => ('url' in e ? [[host, e.url]] : [])));
  }

  /** Queues these hosts unless they have an icon (or failed recently). */
  async want(hosts: Iterable<string>) {
    await this.loaded;
    for (const host of hosts) {
      const entry = this.entries[host];
      if (entry && ('url' in entry || Date.now() - entry.missedAt < RETRY_MISSES_AFTER)) continue;
      if (!this.queue.includes(host)) this.queue.push(host);
    }
    this.pump();
  }

  private pump() {
    while (this.active < CONCURRENCY && this.queue.length) {
      const host = this.queue.shift()!;
      this.active++;
      resolveIcon(host)
        .then((url) => {
          this.entries[host] = url ? { url } : { missedAt: Date.now() };
          if (url) this.onFound({ [host]: url });
        })
        .finally(() => {
          this.active--;
          this.scheduleSave();
          this.pump();
        });
    }
  }

  private scheduleSave() {
    clearTimeout(this.saveTimer);
    this.saveTimer = setTimeout(() => void chrome.storage.local.set({ [STORAGE_KEY]: this.entries }), 1000);
  }
}

/** The site's declared icon (preferring SVG, then apple-touch-icon, then the largest), or /favicon.ico. */
async function resolveIcon(host: string): Promise<string | undefined> {
  const candidates: string[] = [];
  try {
    const res = await fetch(`https://${host}/`, { credentials: 'omit', signal: AbortSignal.timeout(6000) });
    const html = (await res.text()).slice(0, 200_000);
    candidates.push(...declaredIcons(html, res.url));
  } catch {
    // The page may be down or need a login; /favicon.ico can still work.
  }
  candidates.push(`https://${host}/favicon.ico`);

  for (const url of candidates) {
    try {
      const res = await fetch(url, { credentials: 'omit', signal: AbortSignal.timeout(6000) });
      const type = res.headers.get('content-type')?.split(';')[0] ?? '';
      if (!res.ok || !(type.startsWith('image/') || url.endsWith('.ico'))) continue;
      const bytes = new Uint8Array(await res.arrayBuffer());
      if (!bytes.length || bytes.length > MAX_BYTES) continue;
      return `data:${type || 'image/x-icon'};base64,${base64(bytes)}`;
    } catch {
      // Try the next candidate.
    }
  }
  return undefined;
}

function base64(bytes: Uint8Array) {
  let binary = '';
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}
