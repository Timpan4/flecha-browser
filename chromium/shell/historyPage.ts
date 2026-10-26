/**
 * The History page (history.html): the pages you visited, newest first and by day, searchable,
 * each removable, and Chromium's history cleared for a stretch of time. The stretches are the
 * ones Chrome's "Clear browsing data" offers. Clicking a page opens it as a tab in this window.
 */
import './noPageMenu';
import { cmd } from './client';

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const days = $('days');
const search = $<HTMLInputElement>('search');
const more = $<HTMLButtonElement>('more');
const empty = $('empty');

/** What is shown: the entries read so far for the current search, newest first. */
let entries: chrome.history.HistoryItem[] = [];
let exhausted = false;
let reading = 0;

const dayLabel = (time: number) => {
  const day = new Date(time);
  const today = new Date();
  const yesterday = new Date(today.getFullYear(), today.getMonth(), today.getDate() - 1);
  if (day.toDateString() === today.toDateString()) return 'Today';
  if (day.toDateString() === yesterday.toDateString()) return 'Yesterday';
  return day.toLocaleDateString(undefined, { weekday: 'long', year: 'numeric', month: 'long', day: 'numeric' });
};

const favicon = (url: string) => {
  const icon = new URL(chrome.runtime.getURL('/_favicon/'));
  icon.searchParams.set('pageUrl', url);
  icon.searchParams.set('size', '16');
  return icon.href;
};

function row(item: chrome.history.HistoryItem) {
  const li = document.createElement('li');
  const time = Object.assign(document.createElement('time'), {
    textContent: new Date(item.lastVisitTime ?? 0).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' }),
  });
  const link = Object.assign(document.createElement('a'), { href: item.url ?? '', title: item.url ?? '' });
  link.append(Object.assign(document.createElement('img'), { src: favicon(item.url ?? ''), alt: '' }), item.title || item.url || '');
  // Into Flecha's own tabs, in this window, rather than a page Chromium opens on its own.
  link.addEventListener('click', (e) => {
    e.preventDefault();
    cmd('newTab')(item.url);
  });
  const host = Object.assign(document.createElement('span'), { className: 'host', textContent: hostOf(item.url) });
  const remove = Object.assign(document.createElement('button'), { className: 'remove', title: 'Remove from history', textContent: '×' });
  remove.setAttribute('aria-label', `Remove ${item.title || item.url} from history`);
  remove.addEventListener('click', () => void chrome.history.deleteUrl({ url: item.url ?? '' }));
  li.append(time, link, host, remove);
  return li;
}

function hostOf(url = '') {
  // Flecha's own pages (this one, Boosts) are named, not shown as an extension id.
  if (url.startsWith(location.origin + '/')) return 'Flecha';
  try {
    return new URL(url).host.replace(/^www\./, '');
  } catch {
    return '';
  }
}

function render() {
  const groups = new Map<string, chrome.history.HistoryItem[]>();
  for (const item of entries) {
    const label = dayLabel(item.lastVisitTime ?? 0);
    groups.set(label, [...(groups.get(label) ?? []), item]);
  }
  days.replaceChildren(
    ...[...groups].map(([label, items]) => {
      const section = document.createElement('section');
      const list = document.createElement('ul');
      list.append(...items.map(row));
      section.append(Object.assign(document.createElement('h2'), { textContent: label }), list);
      return section;
    }),
  );
  empty.hidden = entries.length > 0;
  more.hidden = exhausted;
}

/** chrome.history.search's default maxResults. */
const PAGE = 100;

/** Reads the next page of entries, from the last one shown back. */
async function readMore() {
  const call = ++reading;
  const last = entries.at(-1)?.lastVisitTime;
  // Up to and including the last one's time, so pages visited in the same instant aren't skipped.
  // Those shown already come back: ask for as many more, so a page of them can't hide the rest.
  const again = last === undefined ? 0 : entries.filter((e) => (e.lastVisitTime ?? 0) < last + 1).length;
  const maxResults = PAGE + again;
  const found = await chrome.history.search({ text: search.value.trim(), startTime: 0, maxResults, ...(last !== undefined ? { endTime: last + 1 } : {}) });
  if (call !== reading) return;
  const known = new Set(entries.map((e) => e.id));
  const fresh = found.filter((e) => !known.has(e.id)).sort((a, b) => (b.lastVisitTime ?? 0) - (a.lastVisitTime ?? 0));
  exhausted = found.length < maxResults || fresh.length === 0;
  entries.push(...fresh);
  render();
}

/** From the newest again: a new search, or history that changed underneath. */
function reload() {
  entries = [];
  exhausted = false;
  void readMore();
}

more.addEventListener('click', () => void readMore());
search.addEventListener('input', reload);
$('clear').addEventListener('submit', async (e) => {
  e.preventDefault();
  const range = $<HTMLSelectElement>('range');
  const label = range.selectedOptions[0].textContent;
  if (!confirm(`Clear your history from ${label}? Pages you have open stay open.`)) return;
  const now = Date.now();
  if (range.value === 'all') await chrome.history.deleteAll();
  else await chrome.history.deleteRange({ startTime: now - Number(range.value), endTime: now });
  reload();
});
chrome.history.onVisited.addListener(reload);
chrome.history.onVisitRemoved.addListener(reload);
reload();
