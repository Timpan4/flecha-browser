import fs from 'node:fs';
import path from 'node:path';
import { randomUUID } from 'node:crypto';
import { app, shell, type DownloadItem, type Session } from 'electron';
import type { DownloadInfo } from '../shared/types';

export interface DownloadOptions {
  folder: string;
  askWhereToSave: boolean;
}

/**
 * Saves downloads straight to the chosen folder (no save dialog, like Chrome) unless
 * "ask where to save" is on, and keeps a list of this session's downloads for the sidebar.
 */
export class Downloads {
  readonly items: DownloadInfo[] = [];
  private live = new Map<string, DownloadItem>();
  private emitTimer?: NodeJS.Timeout;

  constructor(
    session: Session,
    private options: () => DownloadOptions,
    private onChange: (items: DownloadInfo[], started?: DownloadInfo) => void,
  ) {
    session.on('will-download', (_event, item) => this.track(item));
  }

  open(id: string) {
    const info = this.find(id);
    if (info?.state === 'completed') shell.openPath(info.path);
  }

  reveal(id: string) {
    const info = this.find(id);
    if (info && fs.existsSync(info.path)) shell.showItemInFolder(info.path);
  }

  cancel(id: string) {
    this.live.get(id)?.cancel();
  }

  /** Forgets finished downloads; files stay on disk. */
  clear() {
    for (let i = this.items.length - 1; i >= 0; i--) if (this.items[i].state !== 'progressing') this.items.splice(i, 1);
    this.emit();
  }

  private track(item: DownloadItem) {
    const { folder, askWhereToSave } = this.options();
    const savePath = uniquePath(fs.existsSync(folder) ? folder : app.getPath('downloads'), item.getFilename());
    // Without a save path Electron shows its save dialog, starting from the suggested one.
    if (askWhereToSave) item.setSaveDialogOptions({ defaultPath: savePath });
    else item.setSavePath(savePath);
    const info: DownloadInfo = {
      id: randomUUID(),
      filename: path.basename(savePath),
      path: savePath,
      url: item.getURL(),
      received: 0,
      total: item.getTotalBytes(),
      state: 'progressing',
      startedAt: Date.now(),
    };
    this.items.unshift(info);
    this.live.set(info.id, item);

    item.on('updated', (_e, state) => {
      // With the dialog, the real path is only known once the user has picked it.
      if (item.getSavePath() && item.getSavePath() !== info.path) {
        info.path = item.getSavePath();
        info.filename = path.basename(info.path);
      }
      info.received = item.getReceivedBytes();
      info.total = item.getTotalBytes();
      info.state = state === 'interrupted' ? 'interrupted' : 'progressing';
      this.emitSoon();
    });
    item.once('done', (_e, state) => {
      info.received = item.getReceivedBytes();
      info.state = state;
      this.live.delete(info.id);
      if (state === 'completed') app.dock?.downloadFinished(savePath);
      this.emit();
    });
    this.onChange(this.items, info);
  }

  private find(id: string) {
    return this.items.find((d) => d.id === id);
  }

  private emitSoon() {
    this.emitTimer ??= setTimeout(() => this.emit(), 250);
  }

  private emit() {
    clearTimeout(this.emitTimer);
    this.emitTimer = undefined;
    this.onChange(this.items);
  }
}

/** report.pdf → report (1).pdf → report (2).pdf … so nothing is overwritten. */
function uniquePath(dir: string, filename: string) {
  const ext = path.extname(filename);
  const base = path.basename(filename, ext) || 'download';
  let candidate = path.join(dir, base + ext);
  for (let n = 1; fs.existsSync(candidate); n++) candidate = path.join(dir, `${base} (${n})${ext}`);
  return candidate;
}
