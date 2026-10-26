/**
 * Tab sleep, as Dia does it. The rules and numbers are decompiled from Dia 1.54.2
 * (WebContentLifecycleOptimization, ApplicationController.config); the research notes are
 * flecha-perf/findings.md, "Tab lifecycle policy". Times are active use: time the computer is
 * awake, unlocked and in use, except the wall-clock limit.
 */

/** timeToFreezeTab: 1,800 s of active use since the tab was last used. */
export const FREEZE_AFTER = 30 * 60_000;
/** timeToDiscardTab: 14,400 s of active use. */
export const DISCARD_AFTER = 4 * 60 * 60_000;
/** timeToDiscardTabWallClock: 86,400 s, whatever the computer did meanwhile. */
export const DISCARD_AFTER_WALL_CLOCK = 24 * 60 * 60_000;
/** protectedLRUTabCount: the most recently used tabs are only ever frozen by the time rule. */
export const PROTECTED_RECENT = 10;
/** stateDebounceInterval: a tab's state doesn't change again within this. */
export const STATE_DEBOUNCE = 60_000;
/** samplingPeriod: how often the rules run. */
export const EVALUATE_EVERY = 5 * 60_000;

/**
 * Dia's tabSuspensionBlocklist: pages it never suspends. These are the urlRegex values its bundle
 * ships (ARC_WebContentBehaviorConfigurationClient.bundle, web_content_behavior.json, SHA-256
 * 5e492cc2…), decoded into WebContentBehaviorConfiguration.tabSuspensionBlocklist. Matched from
 * the start of the address; the remote disable-tab-suspension-block-list flag has no counterpart.
 *
 * The same file's keepaliveAllowList is not here: Dia doesn't read it (the configuration's
 * CodingKeys are pipDisallowList, pipSiteConfigs and tabSuspensionBlocklist only).
 */
const SUSPENSION_BLOCKLIST = [/^https:\/\/(www|dev|stg)\.notion\.so\//, /^https:\/\/app\.notion\.com\//];

export type SleepState = 'alive' | 'frozen' | 'discarded';

/**
 * Dia's rules see a tab only as an item in a window (OptimizationItem: itemID, windowID), so a
 * pinned tab or a Favorite sleeps like any other.
 */
export interface SleepTab {
  id: string;
  /** The page's address, if known. */
  url?: string;
  /** Wall-clock time it was last used (TabInfo.lastActive). */
  lastActive: number;
  /** Active-use time when it was last used. */
  usedAt: number;
  /** On screen in some window. */
  shown: boolean;
  /** Its Space is on screen in no window. */
  inHiddenSpace: boolean;
  /** Playing media (Dia: "Has media player open"). */
  media: boolean;
  state: SleepState;
  /** Wall-clock time its state last changed, if it has. */
  changedAt?: number;
}

/** The ids of the tabs the time rule only freezes: the most recently used. */
export function protectedRecent(tabs: Array<Pick<SleepTab, 'id' | 'lastActive'>>): Set<string> {
  return new Set([...tabs].sort((a, b) => b.lastActive - a.lastActive).slice(0, PROTECTED_RECENT).map((t) => t.id));
}

/** Dia's "Time Decay" rule (low priority). */
function timeDecay(tab: SleepTab, now: number, activeUse: number, isProtected: boolean): SleepState {
  const idle = activeUse - tab.usedAt;
  if (idle >= DISCARD_AFTER || now - tab.lastActive >= DISCARD_AFTER_WALL_CLOCK) return isProtected ? 'frozen' : 'discarded';
  return idle >= FREEZE_AFTER ? 'frozen' : 'alive';
}

/**
 * The state a tab should be in. Dia's rules by priority: a state that changed within the debounce
 * stays, and a tab with media stays alive (both required); a tab in a Space no window shows is
 * frozen or less alive (medium); otherwise the time rule (low). A tab on screen, or on Dia's
 * blocklist, is always alive. A discarded tab only comes back when it is used, so nothing here
 * brings it back.
 */
export function sleepState(tab: SleepTab, now: number, activeUse: number, isProtected: boolean): SleepState {
  if (tab.shown || tab.media || SUSPENSION_BLOCKLIST.some((re) => re.test(tab.url ?? ''))) return 'alive';
  if (tab.changedAt !== undefined && now - tab.changedAt < STATE_DEBOUNCE) return tab.state;
  const decay = timeDecay(tab, now, activeUse, isProtected);
  const wanted = tab.inHiddenSpace && decay === 'alive' ? 'frozen' : decay;
  return tab.state === 'discarded' ? 'discarded' : wanted;
}
