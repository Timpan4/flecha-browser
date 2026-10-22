export interface TabInfo {
  id: string;
  url: string;
  title: string;
  favicon?: string;
  /** Pinned and favorite tabs remember where they started, like Arc. */
  homeUrl?: string;
  loading?: boolean;
  lastActive: number;
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

export interface BrowserState {
  spaces: Space[];
  activeSpaceId: string;
  favorites: string[];
  tabs: Record<string, TabInfo>;
  archive: ArchivedTab[];
  sidebarVisible: boolean;
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

export interface FlechaApi {
  onState(cb: (state: UiState) => void): void;
  onPalette(cb: (mode: PaletteMode) => void): void;
  onRenameSpace(cb: (spaceId: string) => void): void;
  onPeek(cb: (peek: PeekState | null) => void): void;
  ready(): void;
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
  promotePeek(): void;
}
