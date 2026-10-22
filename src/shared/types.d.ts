export interface TabInfo {
  id: string;
  url: string;
  title: string;
  favicon?: string;
  /** A phone-sized mirror opened by the responsive split. */
  device?: 'mobile';
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
  /** Separate cookies, storage and logins from the other Spaces. */
  isolated?: boolean;
  /** Two tabs shown side by side (left, right) whenever either one is active. */
  split?: [string, string];
  /** `responsive`: the right side is a phone-sized mirror that follows the left side's navigation. */
  splitMode?: 'responsive';
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

export interface EnvironmentTarget {
  label: string;
  origin: string;
}

/** One project's origins, e.g. local → staging → production. */
export interface Environment {
  id: string;
  name: string;
  targets: EnvironmentTarget[];
}

export interface ActiveEnvironment {
  name: string;
  label: string;
  tone: 'local' | 'staging' | 'production';
  /** The same page on the project's other environments. */
  switches: Array<{ label: string; tone: ActiveEnvironment['tone']; url: string }>;
}

/** A dev server found listening on this machine. */
export interface LocalServer {
  port: number;
  url: string;
  /** The project folder it runs in, or the process name. */
  name: string;
}

export type SearchEngineId = 'google' | 'duckduckgo' | 'bing' | 'brave' | 'kagi' | 'ecosia';

export interface BrowserState {
  spaces: Space[];
  activeSpaceId: string;
  favorites: string[];
  tabs: Record<string, TabInfo>;
  archive: ArchivedTab[];
  sidebarVisible: boolean;
  window?: WindowBounds;
  searchEngine?: SearchEngineId;
  /** Where downloads go; the system Downloads folder when unset. */
  downloadsFolder?: string;
  askWhereToSave?: boolean;
  environments?: Environment[];
  /** Built-in ad and tracker blocking; on unless turned off. */
  adBlock?: boolean;
  /** The MCP bridge for coding agents: off unless turned on in Settings. */
  agent?: { enabled: boolean; token: string };
}

export interface UiState extends BrowserState {
  nav: { canGoBack: boolean; canGoForward: boolean };
  searchEngineName: string;
  /** The active Space's colour, darkened in dark mode. */
  tint: string;
  /** Set when the active tab is on one of the configured environments. */
  environment?: ActiveEnvironment;
  localServers: LocalServer[];
  /** Console errors since each tab's last page load (only tabs that have some). */
  consoleErrors: Record<string, number>;
  /** Hosts that have a boost (script or styles), for the sidebar's ⚡ marker. */
  boostedHosts: string[];
  /** Hosts whose icon is cached and served as `favicon://<host>`. */
  faviconHosts: string[];
  /** What a coding agent is doing, for the sidebar. */
  agent?: { connected: boolean; tabId?: string; action?: string; busy: boolean };
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

/** What the settings page shows. */
export interface SettingsView {
  adBlock: boolean;
  searchEngine: SearchEngineId;
  searchEngines: Array<{ id: SearchEngineId; name: string }>;
  /** For display only, with the home folder shortened to ~. */
  downloadsFolder: string;
  askWhereToSave: boolean;
  defaultBrowser: { isDefault: boolean; canSet: boolean };
  /** Edited as text: one `label origin` per line. */
  environments: Array<{ id: string; name: string; targetsText: string }>;
  agent: { enabled: boolean; endpoint?: string; token?: string };
}

/** The downloads folder is only changed through the folder picker, never set directly. */
export type SettingsPatch = Partial<Omit<SettingsView, 'searchEngines' | 'downloadsFolder' | 'defaultBrowser' | 'agent'>> & {
  agentEnabled?: boolean;
};

export interface SettingsApi {
  get(): Promise<SettingsView>;
  set(patch: SettingsPatch): Promise<SettingsView>;
  chooseDownloadsFolder(): Promise<SettingsView>;
  makeDefaultBrowser(): Promise<SettingsView>;
  newAgentToken(): Promise<SettingsView>;
  copy(text: string): Promise<void>;
  openBoosts(): Promise<void>;
}

export interface FlechaApi {
  onState(cb: (state: UiState) => void): void;
  onPalette(cb: (mode: PaletteMode) => void): void;
  /** Main asks the sidebar to open the Space editor (new Space, menu, shortcut). */
  onEditSpace(cb: (spaceId: string) => void): void;
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
  updateSpace(spaceId: string, patch: { name?: string; emoji?: string; color?: string }): void;
  setSpaceIsolated(spaceId: string, isolated: boolean): void;
  deleteSpace(spaceId: string): void;
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
  copyText(text: string): void;
  openSettings(): void;
  toggleResponsive(): void;
  showConsole(tabId: string): void;
  editBoost(kind: 'script' | 'styles'): void;
  openBoostsFolder(): void;
  splitWith(tabId: string): void;
  splitNew(input: string): void;
  closePeek(): void;
  openDownload(id: string): void;
  revealDownload(id: string): void;
  cancelDownload(id: string): void;
  clearDownloads(): void;
  promotePeek(): void;
}
