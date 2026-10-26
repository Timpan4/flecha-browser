# Flecha continuation, 2026-10-25

## Outcome and authority

Continue in this checkout, origin `Timpan4/flecha-browser`, upstream `lucianodiisouza/flecha-browser`, base `cd8f4ab332256ba2d07d592c099f843acec5a45b`. Source edits and focused checks are authorized. Changes below are uncommitted. No push, PR, merge, release, owner-session reload or live helper installation was requested or performed. Old Helium Arcified work stays preserved and paused.

The owner reaffirmed that onboarding/import must work regardless of OS. Keep Arc's everyday UX as the product direction. AI remains deferred, sync excluded, Favorites unrestricted with scrolling, native Profile boundaries and unsaved documents protected.

## Implemented

- Import sources now select Linux, macOS or Windows profile roots, including Helium (`net.imput.helium` on Linux/macOS; `imput/Helium/User Data` under Windows Local AppData). Linux respects XDG config and independent Chrome/Helium config overrides. The owner's real Helium profile was detected with history and no bookmarks; its history was not imported into the owner's Flecha session. Safari and Arc readers remain macOS-only. Custom and sandboxed profile discovery remains open.
- Onboarding uses OS-neutral wording, distinguishes missing helper/discovery errors from an empty result, prevents duplicate pending discovery, and permits retry after failed imports without a success check. Reopening the tour from Settings works and re-runs discovery. Completion saves and awaits extension storage immediately; a native restart test caught the previous debounce losing that transition.
- Added `chromium/scripts/install-native-host.mjs --profile <absolute user-data-dir>`. It uses the existing host bundle, writes only the named profile on POSIX, and registers the required HKCU Chromium native-host key on Windows. Only temporary test-profile installation has run. Windows installation has not run on Windows.
- Linux/Windows shortcuts use Control labels. Tabs use Control-digit; Spaces use Control-Alt-digit. New Space uses Control-Alt-Shift-N, leaving Control-Shift-N to Chromium. macOS defaults retain Command/Control separation. Recording, reset and saved-custom/default conflicts resolve according to native platform modifiers; fixed navigation bindings stay reserved. Flechinha labels and Settings Find use the platform modifier.
- Patch `0054-cross-platform-window-title-branding.patch` changes native window, accessible-window and network-sign-in title branding. It is present in the extracted native source. The exact generated GRIT action and en-US locale repack passed; the isolated X11 window title is `Workflow /a - Flecha`. Only this new patch was reverse-checked with zero fuzz, not a new full patch-stack application.

## Proven checks

`npm run shell:build`, `npm run host:build`, `npm run typecheck`, `npm run typecheck:tsrs`, `node --test chromium/tests/*.test.mjs`, `git diff --check`, and `clang-format --dry-run -Werror chromium/tests/x11-input.cc` pass. There is no configured repository formatter or linter for TypeScript.

All nine regression tests pass. They cover shortcut maps/conflicts, synthetic browser imports, and independent checkout shells/profiles/helpers using one executable. Linux SQLite/bookmark fixtures are read without changing source bytes, including Helium history-only detection and its config override. Windows and macOS path selection is fixture-tested on Linux for Chrome, Brave, Edge, Vivaldi, Opera, Opera GX, Helium and Chromium. These are not Windows/macOS application tests.

The isolated native harness passed sixteen logged checks, including a fresh run against the recovered native build with exit 0. It runs the browser on its own Xvfb display and temporary profile, installs its helper only there, and uses X11 input scoped by that browser PID. It checks missing-helper and failed-import retries, actual onboarding import, Control-digit/Tab/Space keys, Control-L focus and Escape, pins/folders, native split-tab creation, Peek, Flechinha transfer both ways, native title and graceful restart. Draft input, an in-memory counter and document identity survive the in-process operations. Restart assertions cover tour completion, Spaces, pinned-folder structure and tab URLs, not arbitrary live JavaScript or exact document restoration.

Run it from this checkout:

```sh
node chromium/tests/native-workflows.mjs /home/timpan4/.t3/builds/flecha-linux-158.nHjUam/src/out/Dev/chrome /home/timpan4/.t3/builds/flecha-linux-158.nHjUam/ux-evidence
```

It requires the already installed Xvfb, C++ compiler and X11/XTest development libraries. SIGINT/SIGTERM stop its browser. It waits for application events, not fixed sleeps. Optional screenshots wait for finite animations while excluding infinite decorative animation.

Fresh recovery evidence: `/home/timpan4/.t3/builds/flecha-linux-158.nHjUam/workflows-recovery.log`, and isolated `ux-evidence-recovery/onboarding-light.png` / `onboarding-dark.png`. Both fresh captures were visually inspected. They contain only synthetic test state. Earlier `workflows.log` and `ux-evidence` are preserved. No owner desktop capture was made. Shutdown still emits asynchronous "browser is shutting down" console errors; persisted-structure assertions pass, but cleanup remains a follow-up.

Helium adds a seventeenth native check: history-only discovery, onboarding import, direct Chromium history lookup, and unchanged synthetic source bytes. It passed after rebuilding the host. Evidence is `workflows-helium.log` and `ux-evidence-helium/onboarding-{light,dark}.png` beside the native source; both captures were visually inspected. Upstream path references: [Linux](https://github.com/imputnet/helium-linux/blob/main/patches/helium/linux/change-chromium-branding.patch), [Windows](https://github.com/imputnet/helium-windows/blob/main/patches/helium/windows/change-branding.patch).

## Shared runtime and backup

The owner requested both a full backup and reuse across worktrees. `/home/timpan4/flecha-builds/chromium-158-linux/out/Dev` is a complete copy outside T3 and matches the original `out/Dev` by recursive byte comparison. `npm run browser:linux` rebuilds only the calling checkout's shell/helper and uses the shared native executable. Its default profile is that checkout's ignored `.flecha-profile`; `--out` and `--profile` accept explicit absolute paths. It refuses a profile with a SingletonLock. The real launcher smoke test returned `Flecha 158.0.8119.0`. Worktrees need these source changes to have the new npm command. C++ patch changes need a matching native build; the original source tree remains the incremental build location.

A separate verified archive is `/home/timpan4/flecha-backups/chromium-158-linux-20261025/flecha-chromium-158-linux.tar.zst` (5,820,456,737 bytes), with full native outputs, Flecha sources/runtime, original Chromium source archive and patch/build evidence. GNU tar's full source comparison passed, exit 0; SHA-256 is saved in adjacent `SHA256SUMS`. The backup README contains restore instructions. No user browser profiles are included. Both copies are local, not off-machine backups. All seventeen GUI workflows also passed against the shared binary; evidence is `workflows-shared-build.log` beside the preserved source. No native rebuild was run for this work.

## Native build recovery

The authorized recovery is complete. It passed 20,977 actions before the owner's accidental interruption, then all 36,257 remaining actions through `LINK ./chrome`, exit 0. The exact wrapper was run again and returned `ninja: no work to do.`, exit 0. `chrome --version` reports `Flecha 158.0.8119.0`. Its SHA-256 matches the original successful build: `f8be4452258d2004a74c38f71851b8bf6cbc75d338af62443e67bbb1842b3c9d`. Branding resources are separate from this binary.

The recovery was needed because this continuation mistakenly used system Ninja 1.13.2, which rewrote the original build log. Pinned Ninja 1.12.1 then rejected and removed that replacement, losing incremental command metadata. No synthetic log or stamps were created. The owner explicitly authorized the recovery rebuild. Preserve the native source and `out/Dev`; do not clean them or reapply the macOS patch workflow.

From `/home/timpan4/.t3/builds/flecha-linux-158.nHjUam/src`, use only `env -u __PYVENV_LAUNCHER__ third_party/depot_tools/autoninja -C out/Dev chrome`. It resolves to pinned Ninja 1.12.1 and the wrapper's default `-j18`. Recovery evidence is `rebuild-native-recovery.log` beside `src`. After the successful no-op check, `.ninja_log` and `.ninja_deps` were copied together to `ninja-state-after-recovery` beside `src`. No build remains running. Protect this metadata before risky tooling changes.

Requested Opus research completed with verified assignment `claude-opus-5-5`, effort `high`, `fastMode=true`; no delegated work is pending. Its report is `docs/native-build-acceleration-20261025.md`. The recommendation to keep the current toolchain was followed. Cache/compiler changes invalidate work, Siso requires cleaning this output, and sccache rejects its Clang modules. No acceleration was measured. The pinned compiler remains Clang 24.0.0git, revision `e063be79adf127cce301fe5948273102a33012b6`. No GN settings changed.

The owner also requested tsrs integration. Added the latest published `tsrs@0.18.0-ts7.1.0-dev.20260929` as an exact development dependency and `npm run typecheck:tsrs` for both TypeScript projects. That command and the existing `npm run typecheck` pass; list-files checks confirm both project source sets are selected. CONTRIBUTING documents usage. Published platform packages cover Linux and Apple Silicon macOS, not Windows or Intel macOS, so the existing cross-platform TypeScript check remains required. This is a TypeScript checker integration, not a change to Chromium's compilation. No speedup was measured or claimed.

The owner's browser PID 193030 was verified still running after recovery. No owner-session restart, reload or helper installation was performed. Test restarts used only the harness's temporary profile and private display.

## Arc-parity pass, 2026-10-26 (uncommitted)

Items in priority order. "Sol" is the independent GPT 6.1 Sol check.

1. Saved-state robustness (`chromium/shared/savedState.ts`, state version 2): repair by whitelist, last-good and damaged copies, v1 migration. Sol passed after two fix rounds.
2. Peek from any link: "Open in Peek" link menu, Peek in the link's Space partition, focus and scroll kept on close (patch 0055). Sol round 1 failed; both findings fixed.
   - Round 6 (`evidence/round6-24/REPORT.md`, with item 4) passed everything except one case: replacing a Peek still reset the page's scroll and focus.
   - Fixed by patch 0063: `OpenPeekWithContents` focuses the page first. A harness check fails on the pre-0063 runtime (scrollY 500 to 0) and passes with 0063.
   - The round-6 harness timeout was a Ctrl+Shift+T race in the harness. It now waits for the saved close.
   - Round 7 Sol (`evidence/round7-2/REPORT.md`, fast mode) passed A1-A5 and all 45 harness checks. It confirmed the harness check fails on the pre-0063 runtime. It disputed that Ctrl+Shift+T right after a window close is out of reach. Ctrl+Shift+T now waits for window closes the shell is still taking in (`windowsClosing` in background.ts), and the harness presses it at once again. Two runs passed.
   - The real desktop is still unverified: computer-use-linux can't match the test window by PID.
   - Item 4 note: round 6 Sol passed all of item 4 (Control-Tab) on Xvfb. Desktop computer use couldn't find the test window by PID, so the real desktop is unverified.
3. Hidden-toolbar anchors: sidebar extension buttons and popups, plus permission, password and extension bubbles anchored to the sidebar or page area (patch 0056). Sol round 1 failed on a stale anchor; fixed.
   - Round 8 (`evidence/round8-3/REPORT.md`, fast mode) passed everything except one case: clicking a disabled action left a stale anchor.
   - Fixed by patch 0064, which anchors only when `TriggerPopup`'s preconditions hold. The new harness check fails on pre-0064 (right edge 45 instead of 1044) and passes with 0064.
   - App-install placement passed at 126 and 400px. Page info and save-passkey are unverified.
   - The harness cleanup no longer hides a failing step's error.
   - Round 9 Sol (`evidence/round9-3/REPORT.md`, fast mode) passed: the disabled-action cases, the related anchor cases, points 1-4, and all 46 harness checks. Page info and save-passkey are still unverified.
3b. Multi-window, keeping partitions: per-window active Space and Today tabs, global Favorites, closed windows reopen, quit vs. close told apart (patch 0059), windows restored after restart. Sol round 1 found 5 defects; all fixed.
    - The first round-3 re-check ended at model capacity with no report. Its raw log, `flecha-test-shell/evidence/round3-3b/qa-results.log`, showed that a quit within about 139ms of reopening a window lost it. Fixed: `newWindow` and `reopenWindow` now save at once.
    - Round 5 Sol (`evidence/round5-3b/REPORT.md`) failed point 5, everything else passed:
      - A native Ctrl+N window was lost on an immediate quit. Fixed: `onWindowCreated` saves at once.
      - An acknowledged close was resurrected by an immediate quit. Patch 0062 answers `isQuitting(windowId)` false for a window closed before the quit. A service-worker trace shows a forced `Browser.close` still ends the browser 35-50ms after the shell starts its storage write, so that write is lost. This is a general limit, open.
      - Ctrl+Shift+Q did nothing. Chromium 158 has no quit accelerator on desktop Linux; with the app menu hidden, Flecha on Linux has no quit command. Open product gap.
    - The GTK folder-chooser harness step was flaky: keys went out before the dialog had focus. The X11 helper's new `focus` mode fixes it; 3 runs in a row passed.
    - Round 6 Sol (`evidence/round6-3b/REPORT.md`) passed Ctrl+N persistence, patch 0062's decision, and regressions 1, 2 and 6. It retracted the quit defect: Alt+F, then X, quits on Linux. It still fails point 5 under an immediate forced CDP `Browser.close`: storage rejects the write ("The browser is shutting down") and the closed window returns. A menu quit right after a close passes. This forced-exit limit is open.
- The owner's Flecha (PID 712819) ended at 09:28 on 2026-10-26, when the desktop session restarted (sudo systemctl restart from the owner's terminal). It was not caused by this work. `.flecha-profile` still has its stale SingletonLock, untouched.
4. Recent-tab switcher: Control-Tab by recency, committed on modifier release (patch 0057). Sol round 1 failed on a run that began with Control-L; fixed. The round-6 re-check passed (see the note under item 2).
5. Downloads: keep or discard held downloads, resume and retry, download folder chooser and ask-where-to-save in Settings (patch 0058).
   - Round 9 Sol (`evidence/round9-5`) failed one case: at a 126px sidebar, Keep and Discard were clipped.
   - Fixed in CSS: the name column is `minmax(0, 1fr)`, and the action buttons go on their own line.
   - The harness's width check now includes a held download; it fails on the old CSS.
   - Round 10 Sol passed the fix. Dangerous-type holds were not reproducible (.deb and .run were not held).
6. Links from other apps open in Little Arc (Flechinha) on Linux and Windows (patch 0060). Arc for Windows import paths are fixture-tested on Linux. Neither ran on Windows.
   - Round 10 Sol (`evidence/round10-56`) passed 6a-6e except one case, and found two P2 defects.
   - Fixed: a stale empty Arc package hid a valid one. arcWindowsDir now prefers a package with data; there is a test.
   - Fixed: any cold launch with an address lost it; this was not specific to closing windows. Cause: patch 0021 loads the shell asynchronously, and loading an extension with `declarativeNetRequest` cancels in-flight requests; found by bisecting the manifest. The shell now reloads an adopted startup tab whose navigation was cancelled. The harness's last restart launches with an address; the old shell hangs there.
   - Round 11 Sol (`evidence/round11-67`) passed the Arc fix, but the cold-launch fix still stalled sometimes.
   - The shell now re-navigates any adopted startup tab whose address hasn't committed. Fresh-profile probes: 10 of 10 loaded, against 0 of 10 with the round-11 shell. A variant that waited for status complete got only 0-2 of 10.
   - Sol recommended a native fix (load the shell before startup tabs). Not taken.
   - Round-12 re-check running.
7. History page (`history.html`, Control-H or ⌘Y, command bar "Show History"): by day, search, remove a page, read older pages, clear the last hour, 24 hours, 7 days, 4 weeks or all time. Harness-verified.
   - Round 11 Sol: sidebar resizing passed on Xvfb (real-desktop drag unverified).
   - History had two defects, both fixed. Paging lost entries with tied timestamps; it now asks for one page plus the boundary overlap. Ctrl+H from another Space opened a duplicate; it now reuses that window's History tab. Both have harness checks.
   - Round-12 re-check running.
   - Open: the two-window restart check failed once in 9 harness runs, with the first window also showing the second window's Today tab. It didn't recur, and the check now prints both Today lists on failure.
   Resizable sidebar (owner decision): drag the sidebar's right edge between 126 and 400px, Chromium's `kVerticalTabStripUncollapsedMinWidth` and `kVerticalTabStripUncollapsedMaxWidth` in `chrome/browser/ui/tabs/vertical_tab_strip_state.h`.
   - The default stays 260px. The width is saved for all windows.
   - Code: `chromium/shared/sidebarWidth.ts` cites the file, and patch 0061 clamps `setFrame`'s `sidebarWidth` with the constants themselves.
   - At 126px, Favorites, the URL field and the Space switcher fit. The Downloads popover header and the New Tab row needed CSS fixes, now done.
   - On macOS, the toolbar with traffic lights may not fit at 126px. Unchecked.

Checks run after the last change:
- `npm run typecheck`, `npm run typecheck:tsrs` and `git diff --check` pass.
- `node --test chromium/tests/*.test.mjs` passes 35 of 35.
- The native harness passes 49 checks with exit 0 (4 of 4 runs on the final shell), on the incremental `out/Dev` build with patches 0055-0064.
- The script `/home/timpan4/.t3/builds/flecha-perf/tools/mkpatches.sh` verified that patches 0055-0064 apply in order to the originals and reproduce the native tree. It uses the pre-patch copies in `native-pre0061` through `native-pre0064`.
- `x11-input.cc` is clang-format clean.

Testing used `FLECHA_SHELL_OUT=/home/timpan4/.t3/builds/flecha-test-shell/dist` and temporary profiles. `chromium/shell/dist` and `.flecha-profile` belong to the owner's browser; do not touch them.

Sol checks: the owner allows computer use, but only on the checker's own test window; the rules are in `/home/timpan4/.t3/builds/flecha-test-shell/CHECKER.md`. Checks run one at a time because they share one desktop.
- Frozen sources are in `/home/timpan4/.t3/builds/flecha-check/round12`, and the runtime in `flecha-linux-158.nHjUam/checker-runtime/patches-0055-0064`. The round-11 check (item 6 re-check, item 7) is running. Another agent runs perf-bench.mjs on `checker-runtime/patches-0055-0063`; keep that copy.
- After each check, stop any computer-use, test browser or Xvfb processes its Codex app-server left behind, by PID.
- Owner decision, 2026-10-26: Sol checkers started after the items 2+4 check use `target.options` `{ reasoningEffort: "high", serviceTier: "priority" }` (provider codex, model gpt-6.1-sol). Verify that the returned assignment shows serviceTier priority before accepting the work.

Harness runs before the D-Bus fix inherited the desktop session bus, so Chromium may have touched the owner's keyring then. The harness now unsets `DBUS_SESSION_BUS_ADDRESS` and uses `--password-store=basic`.

## Performance work (owner scope, 2026-10-26)

Research: `/home/timpan4/.t3/builds/flecha-perf/findings.md`. Prepared patches are in `flecha-perf/patches` and the bench is `flecha-perf/bench/perf-bench.mjs`. Dia decompilation evidence is in `/home/timpan4/.t3/builds/dia-rea/`.

Owner decisions:
1. shell-01, ad-block hiding as one rule per selector, is taken despite about +5 MB per page process. Applied 2026-10-26; the 603,517-byte cosmetic.css matches the findings. After applying it, typecheck, tsrs, 35 unit tests and the native harness (46 PASS, exit 0) passed.
2. Tab sleep matches Dia's decompiled policy, citing findings.md in the code:
   - Freeze a background tab after 30 min of active use.
   - Discard after 4 h of active use or 24 h of wall clock. The 10 most recently used tabs are only frozen.
   - Freeze tabs of Spaces no window shows.
   - Keep tabs with active media alive.
   - 60 s per-tab debounce; evaluate every 300 s.
   - Active-use time excludes sleep, lock and idle.
   - Low-RAM rule: on moderate pressure with under 200 MB free, discard the shortfall plus 100 MB, then wait 60 s or 600 s.
   - Use Chromium's own discard path so its protections apply; `chrome.tabs.discard` skips them.
   - Memory Saver (0065) on at medium (4 h). 0066 plus shell-04 for hidden-Space freezing.
   - Owner decision, Memory Saver vs Dia's 10-tab protection: match Dia.
     - The shell keeps `autoDiscardable: false` (chrome.tabs.update) on the 10 most recently used tabs, and sets it back to true when a tab leaves that set. Memory Saver and Chromium's time-based discard then skip them, while Dia's rule still freezes them.
     - Harness: a protected tab is not discarded by Memory Saver, and an unprotected one is.
     - Report it if Chromium 158's memory-pressure discard treats the flag differently from Dia's low-RAM rule, which can discard protected tabs under pressure.
   - Report every rule that can't match exactly on Chromium 158.
3. Order:
   1. Finish the Sol checks for items 5, 6 and 7.
   2. Apply shell-02, shell-03, shell-04, 0065, 0066 and the sleep policy. Build incrementally on the existing native tree, with no full or official build.
   3. Extend the harness.
   4. Rerun the perf bench: 3 interleaved runs each, with the findings.md workload.
   5. Run a fast-mode Sol check under CHECKER.md.
   6. Report measured before and after numbers, not estimates.

State (2026-10-26):
- **Shell, applied:** shell-01 to shell-04. shell-04's vote is folded into the sleep policy.
- **Sleep policy:** rules in `chromium/shared/tabSleep.ts` (Dia's decompiled values, cited; 6 unit tests) and `chromium/shell/tabSleep.ts`.
  - Discard goes through Memory Saver's own call. Active use is LiveTicks, counted while chrome.idle says active.
  - The 10 most recent tabs are `autoDiscardable: false`. The policy runs after sync, on a 5-minute alarm and on idle changes.
- **Native:** 0065 and 0066 as prepared; 0067 (`discardTab`, `getLiveTicks`); 0068 (`FlechaLowRamPolicy`, with a new listener tag that rebuilt 751 of 48,406 objects).
- **Checks:**
  - Patches 0055-0068 verify.
  - The harness passes 52/52, adding Memory Saver on at medium, the 10-recent protection against Memory Saver's discard path, and hidden-Space freeze and resume. It shortens Chromium's 5-minute freeze protection to 3 s for the test browser only.
  - Low-RAM probe (`tools/probe25.mjs`, threshold forced, past the 10-minute protection): moderate pressure discarded `autoDiscardable: false` tabs, and kept the form-input tab and the shown tab.
- **Differences from Dia on Chromium 158:**
  - Desktop Linux has no system memory-pressure source (`components/memory_pressure/system_memory_pressure_evaluator.cc`), so real low memory doesn't trigger the low-RAM rule on Linux; only simulated pressure notifications do.
  - Paused media isn't protected; Chromium only protects audible or recently audible pages.
  - Freezing waits Chromium's 5-minute recently-visible protection. Dia's ArcCore uses the same policy, which is an inference.
  - Idle is Chromium's 60 s default.
  - Memory Saver's 4 h timer was not waited for; its discard call was tested instead.
- **Bench:** before = findings baseline (runtime `patches-0055-0063`, `flecha-perf/shell-base`); after = `checker-runtime/patches-0055-0068` plus `flecha-perf/shell-after` (current checkout, which also includes items 2-7). Results go to `flecha-perf/results/r3-*`.

Round-13 limits fixed (owner instruction, 2026-10-26). This supersedes the `autoDiscardable` decision above. `tools/` below is `/home/timpan4/.t3/builds/flecha-perf/tools` (probes, their before/after logs, `probe.mjs`, `mkpatches.sh`, `stages.mjs`); the design notes are `flecha-perf/perf-design.md`. The Dia decompiles for points 3 and 4 are `dia-rea/decomp/dia-decomp-lifecycle.c` and `dia-decomp-blocklist-flag.c`, with their target lists beside them.
1. **Memory Saver vs active use.** New patch `0069-tab-sleep-owns-time-discards` adds `chrome.flechaPrivate.setTabSleepManaged(tabId, managed)`, a per-page mark (`FlechaTabSleepMark`).
   - When Memory Saver's timer fires on a marked page, `MemorySaverModePolicy` does nothing.
   - The shell marks every Flecha tab at each evaluation (a discard makes a new page without the mark) and unmarks tabs that leave Flecha's map.
   - Memory Saver stays on at medium for every other tab (Flechinha, incognito).
   - Chosen over keeping `autoDiscardable:false` on all tabs, which would also hide them from Chromium's pressure discards and from other extensions' choices.
   - The feature `FlechaTabSleepOwnsDiscards` turns the mark off. Its `memory_saver_time_for_testing` parameter shortens Memory Saver's time for probes only and defaults to Chromium's mode time.
   - Evidence: `tools/probe27.mjs` (Memory Saver time 11 min, past Chromium's 10-min recently-visible protection). After the change, the marked tab is kept and the unmarked one discarded (`probe27-after.log`). The old build fails: no such function (`probe27-before.log`).
2. **Restart.** The kept total `tabSleep:activeUse` (local) grows at every sample, so each sample is a heartbeat. The clock checkpoint stays per run (session storage), so a restart adds nothing for the time the browser was closed. LiveTicks keeps running across a browser restart, so counting that gap would age tabs overnight. Dia's clock counts only while Dia runs.
   - Heartbeats come with every evaluation (the 300 s alarm), every change of the tabs on screen and every idle-state change.
   - A restart loses the use since the last heartbeat. That is usually under `EVALUATE_EVERY` (Dia's 300 s samplingPeriod), but it is not a hard limit: Chromium doesn't promise when an alarm is delivered (`extensions/common/api/alarms.webidl`). Sol round 14 (F1) reproduced a 600 s loss by withholding an alarm in the source fixture; no late native alarm was observed.
   - A clock that went back adds nothing.
   - A quit-time sample was tried and dropped: `flechaPrivate` calls fail with "The browser is shutting down" during shutdown (harness log).
   - Unit tests: use up to the last heartbeat survives a restart; 8 h closed doesn't age tabs; a clock that went back adds nothing.
3. **Dia's lists.**
   - `tabSuspensionBlocklist`, the two Notion regexes from `web_content_behavior.json` (SHA-256 5e492cc2…), is decoded into `WebContentBehaviorConfiguration.tabSuspensionBlocklist` (`ipsw/dia-swift.txt`, CodingKeys). It is implemented in `shared/tabSleep.ts`: such tabs stay alive, matched from the start of the address.
   - Dia's consumer of the list couldn't be located statically. Neither the `urlRegex` getter nor the `disable-tab-suspension-block-list` getter has direct callers, and the flag getters are reached indirectly. So "never frozen or discarded" is inferred from the type name. The remote flag has no counterpart.
   - `keepaliveAllowList` is not implemented. Dia's configuration doesn't decode it (CodingKeys are pipDisallowList, pipSiteConfigs, tabSuspensionBlocklist), and the name isn't in the strings extracted from Dia's main binary or ArcCore. Another reader elsewhere isn't ruled out.
4. **Schedule only.** Dia's `WebContentLifecycleController` (0x10099c268) sleeps for the sampling period, optimizes (0x10099c70c) and reschedules (closure 0x10099c61c). Its other callers only restart the timer. The shell now runs the rules on the alarm only. After a sync it calls `shownChanged`, which wakes a frozen tab now on screen and records it as used. The browser reloads a discarded tab itself. Dia's timer restarts from its callers (0x10081f33c, 0x1008202fc, 0x100820ddc) are not mirrored. There is no first evaluation at launch, only at the first alarm, as in Dia.
5. **Pinned/Favorites.** Dia's rules see a tab only as an item (OptimizationItem: itemID, windowID). Its four rules (time decay, non-focused Space, media, recent change) have no pinned or Favorite input. So Flecha keeps treating them like any tab, documented in `SleepTab`, with a unit test.
6. **Low-RAM.** The shell no longer sets `autoDiscardable` at all. Flecha's ten-recent protection lives in its rules only, as Dia's does in its Swift policy. 0068 (amended, unreleased) now honors every Chromium protection, including another extension's `autoDiscardable:false`.
   - Evidence: `tools/probe28.mjs`. After the change, the recent background tab is discarded, the other extension's tab is kept, and incognito is untouched (`probe28-after.log`). Before, the other extension's tab was discarded (`probe28-before.log`; that runtime copy also predates the round-13 off-the-record fix, so its incognito tab was discarded too).
   - The incognito probe `probe26` passes again (`probe26-after.log`).

Checks after these fixes:
- typecheck and tsrs clean; 50/50 unit tests.
- Patches 0055-0069 regenerate and verify (`tools/mkpatches.sh`).
- `out/Dev` incremental build: 389 steps, `build-0069.log` beside `src`.
- Harness 52/52 (`flecha-perf/results/harness-six2.log`) on the shell before the heartbeat change. The rerun on the final shell passed 52/52 (`flecha-perf/results/harness-six3.log`).
- The harness fires the shell's real `tab-sleep` alarm early (`chrome.alarms.create`) instead of relying on sync-time evaluation. It checks that Flecha's tabs are marked and stay auto-discardable.

Remaining differences from Dia:
- Active use is the computer's use (chrome.idle, 60 s), not Dia's app-active state.
- Use between the last heartbeat and a quit or crash is lost (see 2; no hard limit).
- A tab that leaves Flecha's map while hidden, after Memory Saver's timer already fired and was skipped, gets a new Memory Saver timer only at its next visibility change (or a Memory Saver mode change). The shell unmarks it only at the next evaluation.

Sol round 14 (`/home/timpan4/.t3/builds/flecha-test-shell/evidence/round14-perf/REPORT.md`, CDP/Xvfb only): fixes 1, 3, 4, 5 and 6 pass. The three round-13 fixes pass. Typecheck, tsrs, 50 unit tests, the 52-check harness and both long probes pass. One finding, F1 (P2): the 300 s restart-loss bound was stated as strict. Fixed by correcting the claim in `shell/tabSleep.ts` and above. There is no behavior change, and no hard limit exists for alarm delivery.
- The blocklist's exact matching (prefix vs contains) and its precedence among Dia's rules are inferred.

## Official Linux release build (owner scope, 2026-10-26; last in the queue)

This is authorized as a full build. It runs after the item 5-7 Sol checks and after the perf work with its bench and Sol check.
- Location: native tree `/home/timpan4/.t3/builds/flecha-linux-158.nHjUam/src`, new output `out/Release`. `out/Dev` stays untouched, and the two are never built at the same time.
- Args: start from `out/Dev/args.gn` (x64 Linux, proprietary_codecs, ffmpeg_branding "Chrome"). Add is_official_build = true, is_component_build = false, is_debug = false, chrome_pgo_phase = 2 with the Linux profile in `chrome/build/pgo_profiles/`, and symbol_level = 0.
  - Confirm the PGO profile matches the tree's revision and that GN picks it up.
  - Keep every Flecha patch and the shell-dir and branding settings. Nothing macOS-only, such as Sparkle.
  - Record the final args.gn and its source here.
- Build with the pinned depot_tools autoninja and `__PYVENV_LAUNCHER__` unset, in the background with a log.
- Benchmark on its own Xvfb with temporary profiles: Speedometer 3.1 and JetStream (latest stable from browserbench.org).
  - Compare Flecha out/Release, Flecha out/Dev, and Helium (`/opt/helium-browser-bin/helium`, temporary profile; never the owner's Helium profile or running instance).
  - Several interleaved runs each, with other heavy work idle.
  - Report medians with spread and exact versions.
- No commit, push or PR.

Prepared, not yet built. `out/Release/args.gn`, derived from `out/Dev/args.gn`:
```
is_debug = false
is_component_build = false
is_official_build = true
chrome_pgo_phase = 2
dcheck_always_on = false
symbol_level = 0
blink_symbol_level = 0
v8_symbol_level = 0
target_cpu = "x64"
target_os = "linux"
proprietary_codecs = true
ffmpeg_branding = "Chrome"
use_remoteexec = false
treat_warnings_as_errors = false
```
- `gn gen out/Release` made 37,067 targets.
- GN picks up the PGO profile: compile commands carry `-fprofile-use=../../chrome/build/pgo_profiles/chrome-linux-main-1792691882-fcbe222d…profdata`, the file `chrome/build/linux.pgo.txt` names, which is present.
- The tree is `branch-heads/8119@{#1}` (LASTCHANGE 76ebc293…). The profile is from main at fcbe222d; the tree has no git metadata to prove that is 8119's branch point.
- Official defaults also enable `is_cfi` and `use_thin_lto`.
- `flecha_shell_dir` stays empty: it only bundles the shell into the macOS app, and Linux runs with `--flecha-shell-dir`, as Dev does.

Build started 2026-10-26 after Sol round 14, in the background: `env -u __PYVENV_LAUNCHER__ third_party/depot_tools/autoninja -C out/Release chrome`. The log is `build-release.log` beside `src` and ends with `exit N`. It includes patches 0055-0069. It finished at 16:03 with `exit 0`: 57,806 steps, no errors, and `out/Release/chrome` is 549 MB. Against `out/Release` with the test shell, the smoke check passed: Speedometer opens visible in the Flecha tab, reporting Chrome/158.0.8119.0. The native harness passed 52/52 (`flecha-perf/results/harness-release.log`).

Benchmark plan, to run once the build has finished and nothing else heavy runs:
- Runner: `/home/timpan4/.t3/builds/flecha-perf/bench/browserbench.mjs`. `node browserbench.mjs 0 /dev/null` only checks that each browser opens the page. `node browserbench.mjs 5 ../results/browserbench/run-1.jsonl` runs 5 rounds, the owner's "several". 5 keeps for now; change it only if the owner says so.
- Versions: Speedometer 3.1 (`browserbench.org/Speedometer3.1/?startAutomatically=true`) and JetStream 3.0 (`browserbench.org/JetStream3.0/?startAutomatically=true`), the current ones on browserbench.org on 2026-10-26. Helium is 0.20.1.1 (Chromium 156.0.8078.47) at `/opt/helium-browser-bin/helium`.
- Method: each launch gets its own Xvfb (1920x1080) and a fresh temporary profile, with `--disable-gpu` for all three browsers. Flecha opens the benchmark as its active tab after onboarding. Each round runs every benchmark once per browser, with the browser order rotating each round.
- The page area differs by product: 1651x1063 in Flecha next to its sidebar, 1911x1009 in Helium.
- Report the median and spread per browser and benchmark, with each row's `product` string.
- State: run-2 stopped after 9 of 30 runs (owner put the header first; `results/browserbench/run-2.meta`). run-1 is invalid for Speedometer: `#summary` is class "valid" as served, so the runner now waits for `#summary` in the hash and fails on a non-numeric score. Rerun 5 rounds from scratch.

## Inline header and window menu on Linux and Windows (owner priority, 2026-10-26)

- Shell:
  - `ui/index.html`, `ui/styles.css`, `ui/app.ts`: close, minimize and maximize circles where macOS draws the traffic lights, using `chrome.windows`. The circles turn grey when the window isn't focused, and the maximize button reads Restore when maximized. The menu button sits after the sidebar toggle. A sidebar narrower than about 248 px wraps back/forward/reload to a second line instead of spilling.
  - The window menu holds New window, New incognito window, New Flechinha window, History, Downloads, Boosts, Extensions, Find, Zoom (submenu), Print, Developer tools, Import, Settings (Ctrl+,), Help (Keyboard shortcuts, Flecha help), About Flecha, and Quit (Ctrl+Shift+Q). It has Linux shortcut labels, arrow/Home/End/Esc/Tab keys, focus on items, and returns focus to the button.
  - Left out, with what replaces each: Edit (the page's menus and keys), Bookmarks (pins/Favorites), Tab and Window menus (sidebar menus, the window circles), Profiles (none), Check for Updates (Sparkle, macOS only), Hide and Services (macOS only). Task Manager, Open File, Full screen and Save stay on Chromium's keys (Shift+Esc, Ctrl+O, F11); Ctrl+S is Flecha's sidebar toggle.
  - macOS keeps its menu bar and traffic lights, with no menu button: `:root[data-platform='mac']`, and Quit has no default shortcut there.
- Patch 0070: on Aura, a press in a `-webkit-app-region: drag` area stays with the window (`BrowserView::ShouldDescendIntoChildForEventHandling`), so it is a caption: drag moves the window and double-click maximizes, through Chromium's `WindowEventFilterLinux`. Before this the drag areas worked only on macOS. It also adds `flechaPrivate.quit` (`chrome::AttemptUserExit`) and `print` (`chrome::Print`).
- Checks:
  - typecheck and tsrs clean; 51/51 unit tests, including Quit's keys.
  - Patches 0055-0070 verify.
  - The harness passed 58/58 against the installed copy (`flecha-perf/results/harness-installed.log`). New checks: the header layout and labels, a real X11 click in the header gap not reaching the page, the menu's items and keys, Settings from the menu and Ctrl+,, the close circle, and Ctrl+Shift+Q quitting with both windows coming back.
  - Headless Weston (`flecha-perf/tools/probe31.mjs`): maximize, restore and minimize through the circles work on Wayland.
  - Not verified: a window move or a double-click under a real compositor (Xvfb has no window manager, and Weston headless has no input injection); resize edges, which are Chromium's frame borders, unchanged; COSMIC itself.
- Builds: `out/Dev` with 0070 (`build-0070.log`); `out/Release` incremental, 385 steps in 160 s (`build-release-0070.log`).
- Install (`flecha-perf/tools/install-flecha.sh`):
  - The copy is `~/.local/opt/flecha-20261026-1650`, linked from `~/.local/opt/flecha-current`, with the launcher `flecha` and the desktop entry `~/.local/share/applications/flecha.desktop` (Name Flecha, `StartupWMClass=flecha`, http/https/html under "Open with" only; Helium stays the default).
  - The launcher uses `.flecha-profile` (`FLECHA_PROFILE` overrides it) and `--class=flecha`. Each launch registers this copy's import helper in the profile's `NativeMessagingHosts`.
  - Smoke test with a temporary profile (`flecha-perf/tools/smoke-installed.mjs`): the shell loads, the header shows, `WM_CLASS` is `flecha`, and Quit exits with code 0.
- Sol round 15 (`/home/timpan4/.t3/builds/flecha-test-shell/evidence/round15-header/REPORT.md`):
  - Passed: layout, control states, Weston maximize/restore/minimize. In Weston nested on a private Xvfb, a real drag moved the window, double-click maximized and restored, and all four resize edges resized. Every menu action passed, as did Quit with restore, the empty-window rendering and the macOS source review. COSMIC is still unverified: targeted capture was refused.
  - Four P2 findings, all fixed in the shell and harness with no native change:
    1. Leaving a submenu with ← or Esc dropped focus to the body. It now focuses its item (`settingsUi.ts` `leaveSubmenu`).
    2. Submenu items lacked `aria-haspopup` and `aria-expanded`, and their names included "›". The arrow now uses CSS alt text `'›' / ''`, and the submenu is labelled.
    3. The menu's "Import from another browser…" ran the Electron-state import. It now runs `windowUi.ts` `importFromBrowser`: a picker of the browsers the helper detects, then the result. The command bar's action of the same name does too. The Electron import stays as the command-bar action "Import from Flecha (Electron)".
    4. The harness re-selected the first frame when matching restored windows. It now keeps the attached frame's target id.
  - The harness also repeats its `chrome://settings/performance` read when the tab is still committing its navigation (run 2 failed there: `harness-round15fix-run2-context-destroyed.log`).
  - Checks: typecheck and tsrs clean, 51/51 unit tests, `git diff --check` clean. The harness passed 60/60 on `out/Release` (`flecha-perf/results/harness-round15fix.log`). The new checks are submenu ARIA and focus, and Import listing the Chrome and Helium fixtures.
  - Reinstalled as `~/.local/opt/flecha-20261026-1730`; `flecha-current` now points there and the 1650 copy is kept. The installed smoke test passed. The owner's Flecha (PID 3393548, the 1650 copy) was left running; their next launch uses the new copy.

## Remaining gates and next action

0. Sol checks: round 12 (re-check of the round-11 fixes) running; then the performance work, then the official release build.

1. Continue Linux workflow coverage for navigation back/forward, split dragging/resizing, incognito, storage isolation, and exact saved-page/form restoration after restart. Full native Profile isolation is not implemented; current Spaces share history, passwords, extensions and preferences. Shutdown promise cleanup also remains open.
2. Windows/macOS builds, helper installation and application workflows require those platforms. Current source and Linux fixtures do not prove them. Linux/macOS/Windows release packaging remains unproven.
3. Performance: see "Performance work" above.
4. The owner's browser (PID 712819) ended with the desktop session restart at 09:28 on 2026-10-26; verify identity again before any action. Its unsaved state was not operated on. It has not been reloaded onto the new shell. COSMIC still exposes no matching scoped AT-SPI nodes or usable window bounds. Respect the existing targeted-capture refusal; do not capture unrelated desktop content. Installing a helper or changing/reloading the owner's session needs the applicable explicit live-host approval.

## Gap analysis against Arc, Dia and YAB

Owner direction 2026-10-25: feature coverage before speed. Source-only review of all 164 Arcified inventory items: 13 present, 62 partial, 76 missing, 13 not applicable. Summary and priorities: /home/timpan4/.t3/builds/flecha-gap/README.md; per-item tables in the same folder. Inventory: /home/timpan4/.t3/builds/arcified-ux-handoff.md. Speedometer gap (31.5 vs stock Helium 47.4) is attributed to the component Dev build; an official Linux build was declined for now.

Path references used: [Chromium data directories](https://chromium.googlesource.com/chromium/src/+/main/docs/user_data_dir.md), [Firefox profiles](https://support.mozilla.org/en-US/kb/profiles-where-firefox-stores-user-data), [Brave storage](https://support.brave.app/hc/id/articles/29808985123085-Sensitive-data-storage), [Vivaldi profiles](https://help.vivaldi.com/desktop/privacy/preventing-vivaldi-profiles-from-being-uploaded-to-git-repositories/), [native messaging registration](https://developer.chrome.com/docs/extensions/develop/concepts/native-messaging). Native registry/launcher behavior was also inspected in this exact Chromium tree's `launch_context_win.cc` and `chrome_paths.cc`.
