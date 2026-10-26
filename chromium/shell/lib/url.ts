import type { SearchEngineId } from '../../shared/types';

/**
 * `suggest` is each engine's query-suggestion endpoint, as Chromium 158's prepopulated engines
 * have it (third_party/search_engines_data/resources/definitions/prepopulated_engines.json), less
 * the parameters only Chromium fills in. Each answers `["query", ["suggestion", …], …]`.
 */
export const SEARCH_ENGINES: Record<SearchEngineId, { name: string; url: string; suggest: string }> = {
  // Chromium's desktop omnibox asks as client chrome-omni (TemplateURL::GetSuggestionClient).
  google: { name: 'Google', url: 'https://www.google.com/search?q=%s', suggest: 'https://www.google.com/complete/search?client=chrome-omni&q=%s' },
  duckduckgo: { name: 'DuckDuckGo', url: 'https://duckduckgo.com/?q=%s', suggest: 'https://duckduckgo.com/ac/?q=%s&type=list' },
  bing: { name: 'Bing', url: 'https://www.bing.com/search?q=%s', suggest: 'https://www.bing.com/osjson.aspx?query=%s' },
  // Without Chromium's rich=true, which answers with objects instead of strings.
  brave: { name: 'Brave Search', url: 'https://search.brave.com/search?q=%s', suggest: 'https://search.brave.com/api/suggest?q=%s' },
  kagi: { name: 'Kagi', url: 'https://kagi.com/search?q=%s', suggest: 'https://kagisuggest.com/api/autosuggest?q=%s' },
  ecosia: { name: 'Ecosia', url: 'https://www.ecosia.org/search?q=%s', suggest: 'https://ac.ecosia.org/?q=%s&type=list' },
};

export function searchEngine(id: SearchEngineId | undefined) {
  return SEARCH_ENGINES[id ?? 'google'] ?? SEARCH_ENGINES.google;
}

/** The engine's results page for `terms`, even when they look like an address. */
export function searchUrl(terms: string, engine?: SearchEngineId): string {
  return searchEngine(engine).url.replace('%s', encodeURIComponent(terms));
}

/** Turns whatever was typed in the command bar into a URL to load. */
export function resolveInput(raw: string, engine?: SearchEngineId): string {
  const input = raw.trim();
  if (!input) return 'about:blank';
  return webAddress(input) ?? searchUrl(input, engine);
}

/** What was typed as a URL, completed (`github.com` → `https://github.com`); undefined when it reads as a search. */
export function webAddress(raw: string): string | undefined {
  const input = raw.trim();
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(input) || /^(about|data|file):/i.test(input)) return input;
  if (/^(localhost|127\.0\.0\.1|\[::1\])(:\d+)?(\/.*)?$/i.test(input)) return `http://${input}`;
  if (!/\s/.test(input) && /^[^/]+\.[a-z]{2,}(:\d+)?(\/.*)?$/i.test(input)) return `https://${input}`;
  return undefined;
}

/** A URL to keep, for a pinned tab or favorite: typed like the address bar (`github.com` works), never a search. */
export function pinnableAddress(raw: unknown): string | undefined {
  const url = webAddress(String(raw ?? ''));
  if (!url) return undefined;
  try {
    return /^(https?|file|about):$/.test(new URL(url).protocol) ? url : undefined;
  } catch {
    return undefined;
  }
}
