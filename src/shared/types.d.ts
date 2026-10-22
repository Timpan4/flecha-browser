export interface TabInfo {
  id: string;
  url: string;
  title: string;
  favicon?: string;
  /** Pinned and favorite tabs remember where they started, like Arc. */
  homeUrl?: string;
  loading?: boolean;
  lastActive: number;
  /** Back/forward entries (with scroll position) so a tab comes back as it was after a restart. */
  history?: { entries: Array<{ url: string; title: string; pageState?: string }>; index: number };
}

export interface WindowBounds {
  x: number;
  y: number;
  width: number;
  height: number;
  maximized?: boolean;
}

export type PinnedNode =
  | { kind: 'tab'; tabId: string }
  | { kind: 'folder'; id: string; name: string; open: boolean; children: PinnedNode[] };

export interface Space {
  id: string;
  name: string;
  emoji?: string;
  color: string;
  pinned: PinnedNode[];
  today: string[];
  activeTabId?: string;
  /** Two tabs shown side by side (left, right) whenever either one is active. */
  split?: [string, string];
}

export interface ArchivedTab {
  url: string;
  title: string;
  closedAt: number;
}

export interface HistoryEntry {
  url: string;
  title: string;
  visits: number;
  lastVisit: number;
}

export interface DownloadInfo {
  id: string;
  filename: string;
  path: string;
  url: string;
  received: number;
  total: number;
  state: 'progressing' | 'completed' | 'cancelled' | 'interrupted';
  startedAt: number;
}

export interface BrowserState {
  spaces: Space[];
  activeSpaceId: string;
  favorites: string[];
  tabs: Record<string, TabInfo>;
  archive: ArchivedTab[];
  sidebarVisible: boolean;
  window?: WindowBounds;
  /** Built-in ad and tracker blocking; on unless turned off. */
  adBlock?: boolean;
}

export interface UiState extends BrowserState {
  nav: { canGoBack: boolean; canGoForward: boolean };
}

export type PaletteMode = 'new-tab' | 'navigate' | 'split';

export interface PeekState {
  url: string;
  title: string;
  /** Where the peek page sits, so the sidebar UI can draw its toolbar right above it. */
  bounds: { x: number; y: number; width: number; height: number };
}

export type DropTarget =
  | { zone: 'favorites'; beforeTabId?: string }
  | { zone: 'today'; beforeTabId?: string }
  | { zone: 'pinned'; folderId?: string; beforeTabId?: string };

export interface FindResult {
  active: number;
  total: number;
}

/** The find-in-page bar's bridge; it lives in its own small view above the page. */
export interface FindBarApi {
  onOpen(cb: () => void): void;
  onResult(cb: (result: FindResult) => void): void;
  /** `newSearch` starts over (the text changed); otherwise it moves to the next or previous match. */
  find(text: string, forward: boolean, newSearch: boolean): void;
  close(): void;
}

export interface FlechaApi {
  onState(cb: (state: UiState) => void): void;
  onPalette(cb: (mode: PaletteMode) => void): void;
  onRenameSpace(cb: (spaceId: string) => void): void;
  onPeek(cb: (peek: PeekState | null) => void): void;
  /** `started` is set when this update is a new download beginning. */
  onDownloads(cb: (items: DownloadInfo[], started?: DownloadInfo) => void): void;
  ready(): void;
  searchHistory(query: string): Promise<HistoryEntry[]>;
  newTab(input: string): void;
  navigate(input: string): void;
  activate(tabId: string): void;
  close(tabId: string): void;
  back(): void;
  forward(): void;
  reload(): void;
  switchSpace(spaceId: string): void;
  newSpace(): void;
  renameSpace(spaceId: string, name: string): void;
  tabMenu(tabId: string): void;
  spaceMenu(spaceId: string): void;
  toggleFolder(folderId: string): void;
  renameFolder(folderId: string, name: string): void;
  moveTab(tabId: string, target: DropTarget): void;
  clearToday(): void;
  importArc(): void;
  openPalette(mode: PaletteMode): void;
  closePalette(): void;
  toggleSidebar(): void;
  splitWith(tabId: string): void;
  splitNew(input: string): void;
  closePeek(): void;
  openDownload(id: string): void;
  revealDownload(id: string): void;
  cancelDownload(id: string): void;
  clearDownloads(): void;
  promotePeek(): void;
}
