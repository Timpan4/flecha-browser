/**
 * The History page's paging. Chromium's history search picks pages by visit time but reports each
 * page's last visit, so a cursor at the oldest page shown brings back pages shown already (visited
 * before it as well), and a page of those looks like the end of history. Each read asks again
 * from the newest instead, for a page more than the last read asked for.
 */

/** chrome.history.search's default maxResults. */
export const PAGE = 100;

export interface HistoryRead {
  /** Newest first, each page once. */
  entries: chrome.history.HistoryItem[];
  /** That was all of it: the search returned fewer than it was asked for. */
  exhausted: boolean;
  /** How many this read asked for; the next asks for a page more. */
  asked: number;
}

export async function readHistory(
  search: (query: chrome.history.HistoryQuery) => Promise<chrome.history.HistoryItem[]>,
  text: string,
  asked = 0,
): Promise<HistoryRead> {
  const maxResults = asked + PAGE;
  const found = await search({ text, startTime: 0, maxResults });
  // A page can come back twice, for your visits and an agent's (Chromium keeps those apart).
  const entries = [...new Map(found.map((e) => [e.id, e])).values()].sort((a, b) => (b.lastVisitTime ?? 0) - (a.lastVisitTime ?? 0));
  return { entries, exhausted: found.length < maxResults, asked: maxResults };
}
