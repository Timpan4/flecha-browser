import fs from 'node:fs';
import path from 'node:path';
import { app, protocol } from 'electron';
import { writeFileAtomic } from './files';

/**
 * Icons for saved tabs that haven't been opened yet (e.g. everything imported from Arc).
 * Each site's icon is fetched once, straight from the site (no third-party favicon service),
 * cached on disk and served to the sidebar as `favicon://<host>`.
 */

const RETRY_MISSES_AFTER = 7 * 24 * 60 * 60 * 1000;
const MAX_BYTES = 300_000;
const CONCURRENCY = 4;

type Entry = { type: string; data: string } | { missedAt: number };

/** Must run before `app.ready`. */
export function registerFaviconScheme() {
  protocol.registerSchemesAsPrivileged([{ scheme: 'favicon', privileges: { standard: true, secure: true } }]);
}

export class Favicons {
  private file = path.join(app.getPath('userData'), 'flecha-favicons.json');
  private entries: Record<string, Entry> = {};
  private queue: string[] = [];
  private active = 0;
  private saveTimer?: NodeJS.Timeout;

  constructor(private onResolved: () => void) {
    try {
      this.entries = JSON.parse(fs.readFileSync(this.file, 'utf8'));
    } catch {
      // Nothing cached yet.
    }
    protocol.handle('favicon', (request) => {
      const entry = this.entries[new URL(request.url).hostname];
      if (!entry || !('data' in entry)) return new Response(null, { status: 404 });
      return new Response(Buffer.from(entry.data, 'base64'), {
        headers: { 'content-type': entry.type, 'cache-control': 'max-age=86400' },
      });
    });
  }

  /** Hosts with a cached icon; the sidebar only asks for these. */
  hosts() {
    return Object.keys(this.entries).filter((host) => 'data' in this.entries[host]);
  }

  /** Queues every host that has no icon yet (or whose last attempt failed long enough ago). */
  want(urls: Iterable<string>) {
    for (const url of urls) {
      let host: string;
      try {
        const parsed = new URL(url);
        if (!/^https?:$/.test(parsed.protocol)) continue;
        host = parsed.hostname;
      } catch {
        continue;
      }
      const entry = this.entries[host];
      if (entry && ('data' in entry || Date.now() - entry.missedAt < RETRY_MISSES_AFTER)) continue;
      if (!this.queue.includes(host)) this.queue.push(host);
    }
    this.pump();
  }

  private pump() {
    while (this.active < CONCURRENCY && this.queue.length) {
      const host = this.queue.shift()!;
      this.active++;
      resolveIcon(host)
        .then((icon) => {
          this.entries[host] = icon ?? { missedAt: Date.now() };
          if (icon) this.onResolved();
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
    this.saveTimer = setTimeout(() => writeFileAtomic(this.file, JSON.stringify(this.entries)), 1000);
  }
}

const HEADERS = {
  'user-agent': `Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/${process.versions.chrome} Safari/537.36`,
  accept: 'text/html,image/*;q=0.9,*/*;q=0.8',
};

/** The site's declared icon (preferring SVG, then apple-touch-icon, then the largest), or /favicon.ico. */
async function resolveIcon(host: string) {
  const candidates: string[] = [];
  try {
    const res = await fetch(`https://${host}/`, { headers: HEADERS, signal: AbortSignal.timeout(6000) });
    const html = (await res.text()).slice(0, 200_000);
    candidates.push(...declaredIcons(html, res.url));
  } catch {
    // The page may be down or need a login; /favicon.ico can still work.
  }
  candidates.push(`https://${host}/favicon.ico`);

  for (const url of candidates) {
    try {
      const res = await fetch(url, { headers: HEADERS, signal: AbortSignal.timeout(6000) });
      const type = res.headers.get('content-type')?.split(';')[0] ?? '';
      if (!res.ok || !(type.startsWith('image/') || url.endsWith('.ico'))) continue;
      const bytes = Buffer.from(await res.arrayBuffer());
      if (!bytes.length || bytes.length > MAX_BYTES) continue;
      return { type: type || 'image/x-icon', data: bytes.toString('base64') };
    } catch {
      // Try the next candidate.
    }
  }
  return undefined;
}

function declaredIcons(html: string, base: string) {
  const icons: Array<{ href: string; score: number }> = [];
  for (const tag of html.match(/<link\b[^>]*>/gi) ?? []) {
    const attr = (name: string) => tag.match(new RegExp(`${name}\\s*=\\s*["']([^"']+)["']`, 'i'))?.[1];
    const rel = attr('rel')?.toLowerCase() ?? '';
    const href = attr('href');
    if (!href || !rel.includes('icon') || rel.includes('mask')) continue;
    const size = Number(attr('sizes')?.match(/(\d+)/)?.[1] ?? 0);
    const score = href.endsWith('.svg') || attr('type')?.includes('svg') ? 1000 : rel.includes('apple-touch') ? 500 : size;
    try {
      icons.push({ href: new URL(href, base).href, score });
    } catch {
      // Ignore malformed hrefs.
    }
  }
  return icons.sort((a, b) => b.score - a.score).map((i) => i.href);
}
