/**
 * Site icons for tabs that were never opened (pinned tabs after an import, say) and for top
 * sites, as the Electron spike's favicon cache did: Chromium's favicon service, through the
 * extension's `_favicon` endpoint (the "favicon" permission). It answers every page, with a
 * grey globe when it has no icon; a site whose answer is the globe keeps its letter instead.
 */

const TEMPLATE = chrome.runtime.getURL('/_favicon/?size=32&pageUrl=%s');
/** A failed lookup is tried again after this long: the site may have been visited since. */
const RETRY_MS = 10 * 60_000;

const iconFor = (pageUrl: string) => TEMPLATE.replace('%s', encodeURIComponent(pageUrl));

async function bytesOf(pageUrl: string) {
  const response = await fetch(iconFor(pageUrl));
  return new Uint8Array(await response.arrayBuffer());
}

const same = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((byte, i) => byte === b[i]);

export class Favicons {
  /** The endpoint's answer for a page it knows nothing about. */
  private globe = bytesOf('https://flecha.invalid/');
  private hosts = new Set<string>();
  private tried = new Map<string, number>();

  /** Called when hosts were found, so the sidebar can show their icons. */
  constructor(private onFound: () => void) {}

  /** The `faviconUrl` for UiState. */
  readonly url = TEMPLATE;

  list() {
    return [...this.hosts];
  }

  /** Looks up the icons of these pages' sites, once each (again after a while if none was found). */
  async want(urls: string[]) {
    const now = Date.now();
    const pages = new Map<string, string>();
    for (const url of urls) {
      let host: string;
      try {
        if (!/^https?:/.test(url)) continue;
        host = new URL(url).hostname;
      } catch {
        continue;
      }
      if (this.hosts.has(host) || now - (this.tried.get(host) ?? 0) < RETRY_MS || pages.has(host)) continue;
      pages.set(host, url);
    }
    if (!pages.size) return;
    const globe = await this.globe.catch(() => undefined);
    if (!globe) return;
    let found = false;
    await Promise.all(
      [...pages].map(async ([host, url]) => {
        this.tried.set(host, now);
        const bytes = await bytesOf(url).catch(() => undefined);
        if (bytes?.length && !same(bytes, globe)) {
          this.hosts.add(host);
          found = true;
        }
      }),
    );
    if (found) this.onFound();
  }
}
