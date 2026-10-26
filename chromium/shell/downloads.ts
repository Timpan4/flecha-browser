/**
 * Downloads in the Chromium shell, as the Electron version did them: this
 * session's downloads for the sidebar's downloads panel, which opens when one starts.
 * Chromium does the downloading; its own download bubble is turned off, since the sidebar
 * shows them.
 *
 * "This session" is since the browser started (kept in session storage, so it survives the
 * background going to sleep). Clearing erases finished downloads from Chromium's list; the
 * files stay on disk, as in Electron.
 *
 * With Chromium's bubble off, the sidebar also asks about downloads Chromium holds back: a
 * dangerous file, or one sent over plain HTTP (patch 0058 reports and settles both; without it,
 * dangerous files only). The download folder and "ask where to save" are Chromium prefs, which
 * Settings changes through the same patch (settingsHandlers.ts).
 */
import type { DownloadInfo } from '../shared/types';
import { broadcast, handle, onConnect } from './hub';
import { sessionKey } from './incognito';

// Each copy of the shell (regular, incognito) keeps its own.
const SINCE_KEY = sessionKey('downloadsSince');

const since: Promise<string> = (async () => {
  const saved = (await chrome.storage.session.get(SINCE_KEY))[SINCE_KEY] as string | undefined;
  if (saved) return saved;
  const now = new Date().toISOString();
  await chrome.storage.session.set({ [SINCE_KEY]: now });
  return now;
})();

const basename = (file: string) => file.split('/').pop() ?? file;

const native = typeof chrome.flechaPrivate?.getDownloadWarnings === 'function';

/** The held downloads, by id: what the browser reports, or dangerous files from chrome.downloads. */
async function heldDownloads(found: chrome.downloads.DownloadItem[]): Promise<Map<number, DownloadInfo['warning']>> {
  if (native) return new Map((await chrome.flechaPrivate.getDownloadWarnings()).map((w) => [w.id, w.kind]));
  return new Map(found.filter((d) => d.state === 'in_progress' && d.danger !== 'safe' && d.danger !== 'accepted').map((d) => [d.id, 'dangerous']));
}

function toInfo(item: chrome.downloads.DownloadItem, warning?: DownloadInfo['warning']): DownloadInfo {
  const name = basename(item.filename) || basename(new URL(item.finalUrl || item.url).pathname) || 'download';
  return {
    id: String(item.id),
    filename: name,
    path: item.filename,
    url: item.finalUrl || item.url,
    received: item.bytesReceived,
    total: Math.max(0, item.totalBytes),
    state:
      item.state === 'complete' ? 'completed' : item.state === 'in_progress' ? 'progressing' : item.error === 'USER_CANCELED' ? 'cancelled' : 'interrupted',
    startedAt: Date.parse(item.startTime),
    warning,
    canResume: item.canResume,
  };
}

async function items(): Promise<DownloadInfo[]> {
  const found = await chrome.downloads.search({ startedAfter: await since, orderBy: ['-startTime'] });
  const held = await heldDownloads(found);
  return found.map((item) => toInfo(item, held.get(item.id)));
}

let progressTimer: ReturnType<typeof setInterval> | undefined;

/** Sends the list; while something is downloading, again every half second for the progress bars. */
async function emit(started?: chrome.downloads.DownloadItem) {
  const list = await items();
  broadcast('downloads', list, started ? list.find((d) => d.id === String(started.id)) ?? toInfo(started) : undefined);
  const busy = list.some((d) => d.state === 'progressing');
  if (busy && !progressTimer) progressTimer = setInterval(() => void emit(), 500);
  if (!busy && progressTimer) {
    clearInterval(progressTimer);
    progressTimer = undefined;
  }
}

export function registerDownloads() {
  // The sidebar is Flecha's download UI.
  void chrome.downloads.setUiOptions({ enabled: false }).catch(() => undefined);
  chrome.downloads.onCreated.addListener((item) => void emit(item));
  chrome.downloads.onChanged.addListener(() => void emit());
  chrome.downloads.onErased.addListener(() => void emit());
  onConnect(() => void emit());

  const id = (value: string) => Number(value);
  handle({
    // Opening needs a click, so pages open downloads themselves (flechaApi); this is the fallback.
    openDownload: (value: string) => chrome.downloads.open(id(value)),
    revealDownload: (value: string) => chrome.downloads.show(id(value)),
    cancelDownload: (value: string) => chrome.downloads.cancel(id(value)),
    // A click in the sidebar decides, as Chromium's bubble would ask.
    keepDownload: (value: string) => (native ? chrome.flechaPrivate.keepDownload(id(value)) : chrome.downloads.acceptDanger(id(value))).then(() => emit()),
    discardDownload: async (value: string) => {
      if (native) await chrome.flechaPrivate.discardDownload(id(value));
      else {
        await chrome.downloads.cancel(id(value));
        await chrome.downloads.erase({ id: id(value) });
      }
      void emit();
    },
    resumeDownload: (value: string) => chrome.downloads.resume(id(value)).then(() => emit()),
    retryDownload: async (value: string) => {
      const [item] = await chrome.downloads.search({ id: id(value) });
      if (item) await chrome.downloads.download({ url: item.finalUrl || item.url });
    },
    clearDownloads: async () => {
      for (const d of await items()) if (d.state !== 'progressing') await chrome.downloads.erase({ id: id(d.id) });
      void emit();
    },
  });
}
