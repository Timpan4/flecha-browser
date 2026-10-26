/**
 * Tab sleep in the shell; the rules are Dia's (shared/tabSleep.ts).
 *
 * Freezing goes through the browser's freezing votes (patch 0066) and discarding through Memory
 * Saver's own discard (patch 0067), so the browser's protections stay: it neither freezes nor
 * discards a tab that plays sound, captures, holds a call, has form input and so on. A tab the
 * browser won't discard is frozen instead.
 *
 * Active use is the browser's clock that stops while the computer is suspended (base::LiveTicks,
 * patch 0067), counted only while chrome.idle says the computer is in use: time asleep, locked or
 * idle doesn't age a tab. Each of Flecha's tabs is marked as Flecha's to discard (patch 0069), so
 * Memory Saver, whose timer counts awake time, leaves it to these rules; the ten most recently
 * used are kept from discard by the rules alone, as Dia keeps them, so Chromium's own discards
 * under memory pressure don't spare them.
 *
 * As in Dia (WebContentLifecycleController, decompiled 0x10099c268: a task that sleeps for the
 * sampling period, then optimizes and schedules the next), the rules run on a schedule only. A tab
 * that comes on screen is woken at once (the browser reloads a discarded one itself).
 */
import type { Store } from '../shared/store';
import { protectedRecent, sleepState, type SleepState } from '../shared/tabSleep';

export const TAB_SLEEP_ALARM = 'tab-sleep';

/** chrome.idle's default detection interval: idle after 60 s without input. */
const IDLE_AFTER_SECONDS = 60;
/** Active-use time so far, kept across restarts. */
const ACTIVE_USE_KEY = 'tabSleep:activeUse';
/** Each tab's active-use time when it was last on screen, by Flecha tab id. */
const USED_AT_KEY = 'tabSleep:usedAt';
/**
 * The clock at the last sample, and whether the computer was in use then, for this run of the
 * browser only. Each sample is a heartbeat that adds the use up to it to the kept total, so a
 * restart keeps the use up to the last heartbeat and adds nothing for the time the browser was
 * closed, as Dia's clock counts only while it runs. Heartbeats come with every evaluation (the
 * 300 s alarm), every change of the tabs on screen and every change of the idle state, so a
 * restart loses the use since the last of these: usually under EVALUATE_EVERY, but Chromium
 * doesn't promise when an alarm is delivered, so that is not a hard limit. (The browser can't
 * take a sample while it shuts down.)
 */
const CLOCK_KEY = 'tabSleep:clock';
/** What each Chromium tab was last made, and when (this session). */
const CHANGED_KEY = 'tabSleep:changed';
/** The Flecha tabs on screen since the last evaluation (this session). */
const SHOWN_KEY = 'tabSleep:shown';
/** The Chromium tabs marked as Flecha's to discard (this session). */
const MANAGED_KEY = 'tabSleep:managed';

type Changed = Record<number, { state: SleepState; at: number }>;
type Clock = { at: number; state: `${chrome.idle.IdleState}` };

export function tabSleepSupported() {
  const native = chrome.flechaPrivate;
  return (
    typeof native?.setTabFreezing === 'function' &&
    typeof native.discardTab === 'function' &&
    typeof native.getLiveTicks === 'function' &&
    typeof native.setTabSleepManaged === 'function'
  );
}

export function createTabSleep(store: Store, live: Map<string, number>, shownTabIds: () => Set<string>) {
  let work = Promise.resolve();
  /** One at a time: each step reads and writes the same stored records. */
  const queue = (step: () => Promise<unknown>) => {
    work = work.then(step).then(
      () => undefined,
      (err) => console.error('Flecha shell: tab sleep failed', err),
    );
    return work;
  };

  /**
   * Adds the time in use since the last sample of this run; returns the active-use time now. A
   * sample earlier than the last (a clock that went back) adds nothing.
   */
  async function sampleActiveUse(): Promise<number> {
    const [now, state, stored, session] = await Promise.all([
      chrome.flechaPrivate.getLiveTicks(),
      chrome.idle.queryState(IDLE_AFTER_SECONDS),
      chrome.storage.local.get(ACTIVE_USE_KEY),
      chrome.storage.session.get(CLOCK_KEY),
    ]);
    const clock = session[CLOCK_KEY] as Clock | undefined;
    const before = (stored[ACTIVE_USE_KEY] as number | undefined) ?? 0;
    const activeUse = before + (clock?.state === 'active' && now >= clock.at ? now - clock.at : 0);
    await Promise.all([
      chrome.storage.local.set({ [ACTIVE_USE_KEY]: activeUse }),
      chrome.storage.session.set({ [CLOCK_KEY]: { at: now, state } satisfies Clock }),
    ]);
    return activeUse;
  }

  async function apply(chromeId: number, wanted: SleepState): Promise<SleepState> {
    if (wanted === 'discarded') {
      if (await chrome.flechaPrivate.discardTab(chromeId).catch(() => false)) return 'discarded';
      wanted = 'frozen';
    }
    await chrome.flechaPrivate.setTabFreezing(chromeId, wanted === 'frozen').catch(() => undefined);
    return wanted;
  }

  async function evaluate() {
    const activeUse = await sampleActiveUse();
    const now = Date.now();
    const [chromeTabs, stored, session] = await Promise.all([
      chrome.tabs.query({}),
      chrome.storage.local.get(USED_AT_KEY),
      chrome.storage.session.get([CHANGED_KEY, SHOWN_KEY, MANAGED_KEY]),
    ]);
    const byChromeId = new Map(chromeTabs.map((tab) => [tab.id!, tab]));
    const usedAt = (stored[USED_AT_KEY] as Record<string, number> | undefined) ?? {};
    const changed = (session[CHANGED_KEY] as Changed | undefined) ?? {};
    const shown = shownTabIds();
    // On screen since the last evaluation: in use until it left, which was since then.
    const wasShown = new Set((session[SHOWN_KEY] as string[] | undefined) ?? []);
    const shownSpaces = new Set(store.state.windows.map((win) => store.activeSpaceIn(win.id).id));
    const open = [...live].filter(([id, chromeId]) => store.state.tabs[id] && byChromeId.has(chromeId));

    for (const id of Object.keys(usedAt)) if (!store.state.tabs[id]) delete usedAt[id];
    // A tab on screen is in use; one seen for the first time starts aging now.
    for (const [id] of open) if (shown.has(id) || wasShown.has(id) || usedAt[id] === undefined) usedAt[id] = activeUse;
    const kept = protectedRecent(open.map(([id]) => ({ id, lastActive: store.state.tabs[id].lastActive })));

    for (const [id, chromeId] of open) {
      const chromeTab = byChromeId.get(chromeId)!;
      // Marked again each time: a discarded tab comes back as a new page, without the mark.
      void chrome.flechaPrivate.setTabSleepManaged(chromeId, true).catch(() => undefined);
      const space = store.locate(id)?.space;
      const was = changed[chromeId];
      const state: SleepState = chromeTab.discarded ? 'discarded' : was?.state === 'frozen' ? 'frozen' : 'alive';
      const wanted = sleepState(
        {
          id,
          url: chromeTab.url,
          lastActive: store.state.tabs[id].lastActive,
          usedAt: usedAt[id],
          shown: shown.has(id),
          inHiddenSpace: !!space && !shownSpaces.has(space.id),
          media: !!chromeTab.audible,
          state,
          changedAt: was?.at,
        },
        now,
        activeUse,
        kept.has(id),
      );
      if (wanted !== state) changed[chromeId] = { state: await apply(chromeId, wanted), at: now };
    }
    // A tab that is no longer one of Flecha's (gone to a Flechinha, say) goes back to Chromium's
    // defaults: Memory Saver's to discard, and no vote to freeze it.
    const managed = new Set(open.map(([, chromeId]) => chromeId));
    for (const chromeId of (session[MANAGED_KEY] as number[] | undefined) ?? []) {
      if (!managed.has(chromeId) && byChromeId.has(chromeId)) void chrome.flechaPrivate.setTabSleepManaged(chromeId, false).catch(() => undefined);
    }
    for (const [chromeId, record] of Object.entries(changed).map(([key, value]) => [Number(key), value] as const)) {
      if (managed.has(chromeId)) continue;
      if (record.state === 'frozen' && byChromeId.has(chromeId)) void chrome.flechaPrivate.setTabFreezing(chromeId, false).catch(() => undefined);
      delete changed[chromeId];
    }
    await Promise.all([
      chrome.storage.local.set({ [USED_AT_KEY]: usedAt }),
      chrome.storage.session.set({ [CHANGED_KEY]: changed, [SHOWN_KEY]: [...shown], [MANAGED_KEY]: [...managed] }),
    ]);
  }

  /**
   * Wakes the tabs now on screen and remembers them as used, for the next evaluation; a heartbeat
   * for the active-use clock.
   */
  async function shownChanged() {
    await sampleActiveUse();
    const shown = shownTabIds();
    const session = await chrome.storage.session.get([CHANGED_KEY, SHOWN_KEY]);
    const changed = (session[CHANGED_KEY] as Changed | undefined) ?? {};
    const wasShown = new Set([...((session[SHOWN_KEY] as string[] | undefined) ?? []), ...shown]);
    for (const id of shown) {
      const chromeId = live.get(id);
      if (chromeId === undefined || changed[chromeId]?.state !== 'frozen') continue;
      await chrome.flechaPrivate.setTabFreezing(chromeId, false).catch(() => undefined);
      changed[chromeId] = { state: 'alive', at: Date.now() };
    }
    await chrome.storage.session.set({ [CHANGED_KEY]: changed, [SHOWN_KEY]: [...wasShown] });
  }

  return {
    /** Runs the rules (on the alarm). */
    evaluate: () => queue(evaluate),
    /** The tabs on screen changed. */
    shownChanged: () => queue(shownChanged),
    /** The computer went in or out of use: count up to here first. */
    idleChanged: () => queue(sampleActiveUse),
  };
}
