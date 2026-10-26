/**
 * Search suggestions for the command bar, asked for and read the way Chromium's omnibox does
 * (components/omnibox/browser/search_provider.cc and search_suggestion_parser.cc).
 */
import type { SearchEngineId } from '../../shared/types';
import { searchEngine, webAddress } from './url';

/** Chromium's kDefaultMinimumTimeBetweenSuggestQueriesMs (omnibox_field_trial.cc), measured from the last request sent. */
export const SUGGEST_INTERVAL_MS = 100;

/** Chromium's desktop popup holds 8 matches (AutocompleteResult::GetMaxMatches); the typed search is one of them. */
export const MAX_SUGGESTIONS = 7;

/**
 * Whether `query` may be sent to the search engine. Like Chromium's IsQueryPotentiallyPrivate,
 * addresses aren't: nothing with a scheme (chrome://, file:, javascript:, localhost:3000), no web
 * address, and no file path.
 */
export function suggestable(query: string): boolean {
  const q = query.trim();
  if (!q || webAddress(q)) return false;
  if (/^[a-z][a-z0-9+.-]*:\S/i.test(q)) return false;
  // /home/…, ~/…, ./…, \\server\…, C:\…
  if (/^(~|\.{1,2})?[\\/]/.test(q) || /^[a-z]:[\\/]/i.test(q)) return false;
  return true;
}

/**
 * The query suggestions in an engine's answer to `query`, best first. Like Chromium, an answer
 * for another query counts as none, and Google's navigation and calculator entries are left out:
 * they aren't searches.
 */
export function parseSuggestions(body: string, query: string): string[] {
  let root: unknown;
  try {
    // Google's optional XSSI guard: )]}'
    root = JSON.parse(body.replace(/^\)\]\}'[^\n]*\n?/, ''));
  } catch {
    return [];
  }
  if (!Array.isArray(root) || root[0] !== query || !Array.isArray(root[1])) return [];
  const metadata = root[4] && typeof root[4] === 'object' ? (root[4] as Record<string, unknown>) : {};
  const types = Array.isArray(metadata['google:suggesttype']) ? (metadata['google:suggesttype'] as unknown[]) : [];
  const seen = new Set([query.toLowerCase()]);
  const suggestions: string[] = [];
  root[1].forEach((entry: unknown, i) => {
    if (typeof entry !== 'string' || types[i] === 'NAVIGATION' || types[i] === 'CALCULATOR') return;
    const text = entry.trim();
    if (!text || seen.has(text.toLowerCase())) return;
    seen.add(text.toLowerCase());
    suggestions.push(text);
  });
  return suggestions.slice(0, MAX_SUGGESTIONS);
}

/** Asks the engine for suggestions. Rejects when offline or aborted; a failed answer has none. */
export async function fetchSuggestions(
  engine: SearchEngineId | undefined,
  query: string,
  signal: AbortSignal,
  load: (url: string, init: RequestInit) => Promise<Response> = (url, init) => fetch(url, init),
): Promise<string[]> {
  // Without cookies: Chromium sends them but never stores what comes back
  // (LOAD_DO_NOT_SAVE_COOKIES), which fetch can't do, so none go either way.
  const response = await load(searchEngine(engine).suggest.replace('%s', encodeURIComponent(query)), { signal, credentials: 'omit' });
  if (!response.ok) return [];
  return parseSuggestions(decode(await response.arrayBuffer(), response.headers.get('content-type')), query);
}

/** Some engines answer in the charset their Content-Type names, not UTF-8 (SearchSuggestionParser::ExtractJsonData). */
function decode(bytes: ArrayBuffer, contentType: string | null) {
  const charset = /charset=["']?([^;"'\s]+)/i.exec(contentType ?? '')?.[1];
  try {
    return new TextDecoder(charset ?? 'utf-8').decode(bytes);
  } catch {
    return new TextDecoder().decode(bytes);
  }
}

export interface SuggestClock {
  now(): number;
  setTimeout(run: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

const realClock: SuggestClock = {
  now: () => Date.now(),
  setTimeout: (run, ms) => setTimeout(run, ms),
  clearTimeout: (handle) => clearTimeout(handle as ReturnType<typeof setTimeout>),
};

/**
 * Asks for suggestions as the query changes; call it with each new query, or with undefined to
 * stop. Only the latest query's suggestions reach `show`: a new call aborts the request in flight
 * and drops any answer still to come. Requests go at most every SUGGEST_INTERVAL_MS. Failures
 * show nothing.
 */
export function suggestionRequests(
  load: (query: string, signal: AbortSignal) => Promise<string[]>,
  show: (query: string, suggestions: string[]) => void,
  clock: SuggestClock = realClock,
) {
  let timer: unknown;
  let current: AbortController | undefined;
  let lastSent = -Infinity;
  return (query: string | undefined) => {
    clock.clearTimeout(timer);
    current?.abort();
    current = undefined;
    if (!query) return;
    const controller = (current = new AbortController());
    const send = () => {
      lastSent = clock.now();
      load(query, controller.signal).then(
        (suggestions) => current === controller && !controller.signal.aborted && show(query, suggestions),
        () => undefined,
      );
    };
    const wait = SUGGEST_INTERVAL_MS - (clock.now() - lastSent);
    if (wait > 0) timer = clock.setTimeout(send, wait);
    else send();
  };
}
