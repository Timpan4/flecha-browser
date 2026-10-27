/**
 * Incognito windows. The shell runs in split incognito mode ("incognito": "split" in the
 * manifest): incognito windows get a second copy of the shell, with its own background,
 * that only sees incognito windows and tabs, the way Chromium opens each window's frame page
 * in the window's own profile (patch 0032). That copy keeps one Space of tabs, in memory
 * only, and forgets it when the last incognito window closes.
 *
 * Both copies share chrome.storage, so the incognito one keeps its keys apart (sessionKey)
 * and only reads the regular state, for your preferences (theme, search engine, shortcuts…).
 *
 * What an incognito window leaves behind: nothing in chrome.storage.local, which is on disk.
 *  - Tabs, console error counts (diagnostics) and this session's downloads list: in
 *    chrome.storage.session, in memory, under the incognito copy's own keys.
 *  - Site icons it fetches: in memory only (siteIcons.ts). Tab sleep doesn't run there.
 *  - Boosts apply to its pages, but it writes none, since a boost names its site: editing one
 *    opens the Boosts page in a regular window (features/boosts.ts).
 *  - Screenshots (capturePage) go to the clipboard only, as in a regular window.
 *  - The coding agent's bridge and imports run in the regular copy only, and that copy's calls
 *    never reach incognito windows or tabs (flechaPrivate.d.ts), so an agent can't see them.
 */
import type { BrowserState } from '../shared/types';

/** Whether this page or background belongs to the incognito profile. */
export const incognito = !!chrome.extension?.inIncognitoContext;

/** A chrome.storage.session key of this copy of the shell's own. */
export const sessionKey = (key: string) => (incognito ? `incognito:${key}` : key);

/** The one Space of an incognito window. */
export const INCOGNITO_SPACE_ID = 'incognito';

/** Preferences the incognito windows take from the regular state, and keep following. */
const FOLLOWED = ['theme', 'searchEngine', 'keybindings', 'adBlock', 'environments', 'peekPinnedLinks', 'sidebarInPageFullscreen'] as const;

/** The regular state's preferences, as a patch onto an incognito state. */
export function followedPreferences(regular: BrowserState | undefined): Partial<BrowserState> {
  const prefs: Partial<BrowserState> = {};
  for (const key of FOLLOWED) if (regular?.[key] !== undefined) (prefs as Record<string, unknown>)[key] = regular[key];
  return prefs;
}

/** A fresh incognito state: one Space, no tabs, your preferences. */
export function incognitoState(regular: BrowserState | undefined): BrowserState {
  return {
    spaces: [{ id: INCOGNITO_SPACE_ID, name: 'Incognito', emoji: '🕶️', color: '#ddd5f3', pinned: [] }],
    // Each incognito window gets its own record (a new one shows this Space too).
    windows: [{ id: crypto.randomUUID(), activeSpaceId: INCOGNITO_SPACE_ID, spaces: {} }],
    closedWindows: [],
    favorites: [],
    tabs: {},
    archive: [],
    sidebarVisible: regular?.sidebarVisible ?? true,
    // Nothing to archive: the tabs go when the window does.
    archiveAfterHours: 0,
    // No welcome tour here.
    onboarded: true,
    // Links from other apps never land in an incognito window.
    openLinksInFlechinha: false,
    ...followedPreferences(regular),
  };
}
