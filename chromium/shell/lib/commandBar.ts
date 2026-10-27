/**
 * How the command bar orders what it finds: one ranked list, as Arc's Command Bar shows it,
 * with no group headings. Arc doesn't publish its ranking; this follows what The Browser
 * Company's own Command Bar demo shows (start.arc.net/master-multitasking, command-bar.mov):
 *  - the first row is the top hit, and is selected;
 *  - an open tab whose title starts with what's typed, an action that starts with it ("pin to
 *    g" → Pin to Work), or the page the address autocompletes to, comes before the typed search;
 *  - the typed search comes next, then the engine's suggestions, then looser matches.
 * Ties go to the most recently used.
 */

/** What an empty Command Bar shows in Arc's demo: the three most recent other tabs, then two actions. */
export const EMPTY_TABS = 3;
export const EMPTY_ACTIONS = 2;

export type ResultKind = 'completion' | 'address' | 'developer' | 'action' | 'tab' | 'search' | 'suggestion' | 'history' | 'archive';
export type Match = 'start' | 'word' | 'contains';

const words = (text: string) => text.toLowerCase().split(/[^\p{L}\p{N}]+/u).filter(Boolean);

/** How well `query` matches: the text starts with it, every word of it starts a word, or it appears anywhere. */
export function matchQuality(query: string, ...texts: string[]): Match | undefined {
  const q = query.trim().toLowerCase();
  if (!q) return undefined;
  const lower = texts.map((t) => t.toLowerCase());
  if (lower.some((t) => t.startsWith(q))) return 'start';
  const have = lower.flatMap(words);
  const want = words(q);
  if (want.length && want.every((w) => have.some((h) => h.startsWith(w)))) return 'word';
  if (lower.some((t) => t.includes(q))) return 'contains';
  return undefined;
}

const SCORES: Record<ResultKind, Partial<Record<Match | 'any', number>>> = {
  completion: { any: 1500 },
  // A typed address, or a developer shortcut (`gh 12`, `env`, a JWT): what was typed on purpose.
  address: { any: 1450 },
  developer: { any: 1450 },
  action: { start: 1400, word: 1250 },
  tab: { start: 1300, word: 1150, contains: 950 },
  search: { any: 1200 },
  history: { start: 1150, word: 1050, contains: 900 },
  archive: { start: 1100, word: 1000, contains: 850 },
  suggestion: { any: 1020 },
};

/** A result's place in the list; undefined when it doesn't match well enough to show. */
export function score(kind: ResultKind, match?: Match): number | undefined {
  const table = SCORES[kind];
  return table.any ?? (match ? table[match] : undefined);
}

export interface Ranked {
  score: number;
  /** Most recent use (ms), for ties. */
  recency?: number;
  /** Results for the same page keep only the best one (an open tab beats its history entry). */
  url?: string;
}

/** Best first; one result per page. Equal scores keep the order given, after recency. */
export function rank<T extends Ranked>(results: T[]): T[] {
  const sorted = results
    .map((result, index) => ({ result, index }))
    .sort((a, b) => b.result.score - a.result.score || (b.result.recency ?? 0) - (a.result.recency ?? 0) || a.index - b.index)
    .map(({ result }) => result);
  const seen = new Set<string>();
  return sorted.filter((r) => !r.url || (!seen.has(r.url) && (seen.add(r.url), true)));
}

/** `github.com/foo` for https://www.github.com/foo/: what a row shows after the title. */
export function displayUrl(url: string): string {
  try {
    const u = new URL(url);
    if (u.protocol !== 'http:' && u.protocol !== 'https:') return url;
    return (u.host.replace(/^www\./, '') + u.pathname + u.search).replace(/\/$/, '');
  } catch {
    return url;
  }
}

/**
 * What to ask history for. Chromium's history search matches whole words, so a partly typed
 * address ("example.o", "github.com/fo") finds nothing; its first word finds the pages, and the
 * bar then keeps those that match all of what's typed.
 */
export function historyQuery(query: string): string {
  if (/\s/.test(query)) return query;
  return query.split(/[^\p{L}\p{N}-]+/u).find(Boolean) ?? query;
}

export interface Visited {
  url: string;
  title?: string;
  /** Visit count or recency; the most used site wins. */
  weight?: number;
}

/**
 * Inline autocomplete: the address `typed` is the beginning of, among pages you've visited
 * ("1pass" → "1password.com", the rest selected). A site name completes to the site; a longer
 * address to the page. Undefined when there is nothing to add, or `typed` reads as a search.
 */
export function inlineCompletion(typed: string, visited: Visited[]): { text: string; url: string; title?: string } | undefined {
  const lower = typed.toLowerCase();
  if (!lower || /\s/.test(lower) || lower.includes('://')) return undefined;
  let best: { text: string; url: string; title?: string; weight: number; root: boolean } | undefined;
  for (const page of visited) {
    let u: URL;
    try {
      u = new URL(page.url);
    } catch {
      continue;
    }
    if (u.protocol !== 'http:' && u.protocol !== 'https:') continue;
    const host = u.host.replace(/^www\./, '');
    const full = displayUrl(page.url);
    const root = full === host;
    let candidate: { text: string; url: string; title?: string } | undefined;
    if (host.startsWith(lower)) candidate = { text: host, url: `${u.protocol}//${u.host}/`, title: root ? page.title : undefined };
    else if (full.startsWith(lower)) candidate = { text: full, url: page.url, title: page.title };
    if (!candidate || candidate.text.length <= lower.length) continue;
    const weight = page.weight ?? 0;
    // The site's own front page names the site best; otherwise the most used.
    if (!best || (root && !best.root) || (root === best.root && weight > best.weight)) best = { ...candidate, weight, root };
  }
  return best && { text: best.text, url: best.url, title: best.title };
}
