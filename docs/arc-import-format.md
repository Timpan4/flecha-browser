# Arc import format

How Arc stores the data Flecha's Arc import reads (`chromium/native-host/arcImport.ts`), how sure we are of each part, and what still needs checking against a real install. Arc for macOS and Windows stops working on 2026-12-22 (issue #7).

Research date: 2026-10-27. No real Arc file was read for this work. Every field below comes from the open-source importers listed under [Sources](#sources), and Flecha's tests use a synthetic fixture built from this document (`chromium/tests/arc-fixture.mjs`).

Confidence levels used below:

- **Confirmed**: read the same way by at least two maintained importers, or by one maintained importer and Arc's own help pages.
- **Single source**: one maintained importer reads it. Flecha uses it, and it is listed for checking.
- **Unverified**: no maintained importer reads it and no real file was seen. Flecha doesn't build on it.

## Files

| File | macOS | Windows | Confidence |
| --- | --- | --- | --- |
| `StorableSidebar.json` | `~/Library/Application Support/Arc/` | `%LOCALAPPDATA%\Packages\TheBrowserCompany.Arc_<publisher id>\LocalCache\Local\Arc\` | macOS confirmed (all sources); Windows single source (arc-export-windows) |
| `StorableArchiveItems.json` | Same folder as the sidebar | Assumed beside the sidebar | macOS confirmed; Windows unverified (arcaeologist guesses `%LOCALAPPDATA%\Arc\User Data\` and marks it untested) |
| `User Data/Local State` | `Arc/User Data/Local State` | Same, under the package folder | Confirmed (rafcabezas/arc2zen reads profile names from it) |

The Windows package path has never been checked by Flecha on a Windows machine (`browserImport.ts` `arcWindowsDir`).

## StorableSidebar.json

Read from `sidebar`, not `sidebarSyncState`: ChromeSideBar documents `sidebarSyncState` as the stale CloudKit copy. **Confirmed.**

`sidebar.containers` holds a `{ global: … }` entry and one entry with `spaces`, `items` and `topAppsContainerIDs`. `spaces` and `items` are flat arrays alternating an id string and its object. **Confirmed.**

### Spaces

| Field | Shape | Confidence | Flecha |
| --- | --- | --- | --- |
| `title` | string | Confirmed | Space name |
| `customInfo.iconType.emoji_v2` | emoji string | Confirmed | Space emoji |
| `customInfo.iconType.icon` | Arc icon name | Confirmed (ChromeSideBar, rafcabezas/arc2zen) | Not read: Flecha Spaces have emoji only |
| `containerIDs` | `["pinned", <id>, "unpinned", <id>]`, either order | Confirmed | Pinned and Today tabs |
| `newContainerIDs` with object markers (`{ "pinned": … }`) | Same pairs | Single source (olegfour3/arc2zen; arcaeologist also accepts object markers in `containerIDs`) | Read when `containerIDs` is missing |
| `customInfo.windowTheme.primaryColorPalette.midTone` | `{ red, green, blue, alpha, colorSpace: "extendedSRGB" }`, channels 0–1 but able to go past either end | Confirmed (ChromeSideBar, rafcabezas/arc2zen) | Space colour, clamped to 0–1 and written as `#rrggbb` |
| Other `windowTheme` fields (gradient stops, noise, light/dark variants) | Unknown | Unverified | Not read |
| `profile` | `{ "default": true }` or `{ "custom": { "_0": { "directoryBasename": "Profile 1", "machineID": … } } }` | Confirmed (rafcabezas/arc2zen, olegfour3/arc2zen, arcaeologist) | Recorded in the summary; offers Separate Cookies and Logins |

The `midTone` is the colour the two importers use. rafcabezas/arc2zen notes that Arc shows a lighter tint of it in the window, so Flecha's Space may look more saturated than the Arc Space did. Flecha's own dark-mode darkening (`chromium/shell/lib/spaceTint.ts`) still applies.

Profile names come from `User Data/Local State` → `profile.info_cache[<directoryBasename>].name`. When the name is missing, Flecha shows the folder name.

### Items

| `data` key | Meaning | Confidence | Flecha |
| --- | --- | --- | --- |
| `tab` | `{ savedURL, savedTitle, timeLastActiveAt, … }`; the item's `title` is a name the user gave it | Confirmed | Tab. Only `http(s)` addresses; others (such as `arc:`) are counted as skipped |
| `list` | Folder; `childrenIds` in display order | Confirmed | Folder |
| `list.automaticLiveFolderData` | Live folder filled at run time (GitHub PRs and similar) | Single source (ChromeSideBar) | Imported as an ordinary folder with whatever children are stored, usually none |
| `itemContainer` | The pinned, unpinned and favorites containers | Confirmed | Walked, never shown |
| `splitView` | A split; its `childrenIds` are the tabs in it | Single source (rafcabezas/arc2zen's fixture and its skipped-kind report), consistent with Arc's help page, where a split becomes its own sidebar item | Split, see below |
| Children under a `tab` item | Tabs nested under a tab | Single source (rafcabezas/arc2zen) | Placed after that tab |
| Any other key (`easel`, `welcomeToArc` and others) | Arc features Flecha doesn't have | Unverified as a list | Counted by key in the preview's "What won't come over"; their children are kept |

Split layout fields (orientation, sizes) are unverified and not read. Flecha uses only the order of the split's tabs.

### Favorites

`topAppsContainerIDs` alternates a profile marker (shaped like a Space's `profile`) and the id of that profile's favorites container. **Confirmed** (arcaeologist, rafcabezas/arc2zen). Flecha's favorites are shared by all Spaces, so the favorites of every profile are imported, each address once.

## StorableArchiveItems.json

`items` alternates an id and an entry. **Confirmed for macOS** (arcaeologist, rafcabezas/arc2zen `restore_auto_archived.py`).

| Field | Shape | Flecha |
| --- | --- | --- |
| `archivedAt` | Seconds since 2001-01-01 UTC (Apple reference date) | `closedAt` = (`archivedAt` + 978307200) × 1000 |
| `reason` | `"auto"` (timer) or `"manual"` (closed) | Both imported, since Flecha's archive holds closed tabs too |
| `sidebarItem` | Same shape as a sidebar tab item | `url` from `data.tab.savedURL`, `title` from the item's `title`, else `savedTitle` |
| `source.space._0` | The Space's id | Not used: Flecha's archive isn't per Space |

Each address is kept once, at its latest archiving. Flecha's archive holds the 500 most recent entries (`ARCHIVE_LIMIT` in `chromium/shared/store.ts`); the preview and the summary say how many didn't fit.

## Little Arc

**Unverified, not imported.** None of the importers reads Little Arc, and no source names a file for it. Arc's help page describes Little Arc windows as temporary and archived on a timer. If archived Little Arc tabs are written to `StorableArchiveItems.json`, Flecha imports them with the rest of the archive, since it doesn't filter by `source`. That is not confirmed.

## Notes

**Can't be read from disk, not imported.** Arc's help page says Notes were phased out in 2024, with Help › Export Arc Notes producing a zip of HTML files, and archived notes listed in the Library under "Easels & Notes". No importer reads Notes, and no source names a file or database holding them. Flecha's preview and import summary say so and point to Arc's own export. Whether that export still works in current Arc builds is unverified.

## Mapping to Flecha

| Arc | Flecha |
| --- | --- |
| Space name, emoji | Space name, emoji (Flecha picks an emoji when Arc used an icon) |
| Space colour (`midTone`) | Space colour |
| Pinned tabs and folders | Pinned tabs and folders |
| Unpinned tabs | Today tabs in the window that imports |
| Favorites of every profile | Favorites, each address once |
| A split of two tabs | The Space's split when both tabs are in the Space after the import and the Space has no split yet |
| Other splits (a second one in a Space, more than two tabs, or a tab that didn't come over) | Their tabs, next to each other where the split was |
| Archived tabs | Archive, with title, address and time |
| Profile other than Arc's default | Listed in the summary; the preview offers Separate Cookies and Logins for its Spaces (on by default), since Flecha has no Profiles yet (#9) |
| Little Arc, Notes, Easels, Boosts | Not imported; listed in the preview and summary |

Merge and Add as new:

- **Merge into matching Spaces** joins an imported Space to the Flecha Space of the same name (ignoring case and surrounding spaces). A tab is left out when that Space's pinned or Today tabs, or the favorites, already have its address. Folders of the same name are joined. The existing Space keeps its own colour and emoji.
- **Add as new Spaces** adds every imported Space, even when the name is taken, as Arc had it.
- Either way, a favorite is never added twice, and an archive entry with the same address and time is never added twice.

## Known limits

- Supported Arc versions: unknown. None of the sources states the Arc version it was written against, and Flecha has not been checked against any Arc build.
- Everything Arc-specific in Flecha's import is tested only with the synthetic fixture: Space colour, Profiles and their names, splits, favorites per profile, the archive, `newContainerIDs`, skipped kinds, and Merge and Add as new.
- The Windows paths have never run on Windows. arc-export-windows repairs `C:\` paths and `https:\/\/` in the Windows sidebar file before parsing, which suggests the Windows file may sometimes not be valid JSON. That is unverified; Flecha doesn't repair the file and would report it as unreadable.
- Arc rewrites its files while running. A copy taken mid-write can be incomplete (rafcabezas/arc2zen retries once). Flecha reads once and reports a parse failure as a warning, so quitting Arc first is the safe advice.

## Verifying against a real install

Someone with Arc on macOS or Windows can confirm or correct this document without sharing URLs or titles. Run these with [jq](https://jqlang.org/) in Arc's folder (paths above) and send the output, with Arc's version from Arc › About Arc and the OS version:

1. The folder's file names, to look for Notes or Little Arc files:
   `ls -la` (macOS) or `dir` (Windows)
2. Item kinds and how often each occurs:
   `jq '[.sidebar.containers[] | objects | .items[]? | objects | (.data // {} | keys[0]) // "none"] | group_by(.) | map({kind: .[0], count: length})' StorableSidebar.json`
3. Each Space's container keys, profile and colour, without its name:
   `jq '[.sidebar.containers[] | objects | .spaces[]? | objects | {keys: keys, profile, midTone: .customInfo.windowTheme.primaryColorPalette.midTone, themeKeys: (.customInfo.windowTheme // {} | keys)}]' StorableSidebar.json`
4. The shape of split items:
   `jq '[.sidebar.containers[] | objects | .items[]? | objects | select(.data.splitView) | {splitView: .data.splitView, children: (.childrenIds | length)}]' StorableSidebar.json`
5. The favorites pairs:
   `jq '[.sidebar.containers[] | objects | .topAppsContainerIDs? // empty | .[] | if type == "string" then "<id>" else . end]' StorableSidebar.json`
6. The archive's shape, without addresses:
   `jq '[.items[] | objects | {keys: keys, reason, archivedAt, source, itemKeys: (.sidebarItem | keys)}] | .[:5]' StorableArchiveItems.json`
7. Windows only: the full path of the folder holding `StorableSidebar.json`, whether `StorableArchiveItems.json` is beside it, and whether `jq . StorableSidebar.json > /dev/null` (or `jq . StorableSidebar.json > NUL`) reports a parse error.
8. A screenshot of Arc's Space switcher next to the same Spaces after importing into Flecha, to compare colours.

The output holds ids, profile folder names and Arc's `machineID`, but no addresses or titles. Replace the `machineID` values before sending if you prefer.

Then run the import in Flecha on that machine and compare the preview's counts per Space with Arc's sidebar.

## Sources

Commits are the ones read on 2026-10-27.

- [monemone-org/ChromeSideBar](https://github.com/monemone-org/ChromeSideBar) at `90d164f` (2026-09-20): `src/utils/arcImport.ts`, `docs/features/completed/029-import-arc-sidebar.md`. Sidebar layout, `sidebarSyncState`, item kinds, Space colour.
- [rafcabezas/arc2zen](https://github.com/rafcabezas/arc2zen) at `f5d7da9` (2026-10-23): `sync_arc_to_zen.py`, `restore_auto_archived.py`, `tests/test_sync.py`. Profiles and their names, colour, favorites per profile, `splitView`, nested tabs, archive.
- [plainspace/arcaeologist](https://github.com/plainspace/arcaeologist) at `e098867` (2026-07-24): `src/parser.js`, `README.md`. Archive format and dates, profile markers, favorites per profile, object markers.
- [olegfour3/arc2zen](https://github.com/olegfour3/arc2zen) at `b3a0017` (2026-07-14): `core/arc_exporter.py`, `.cursor/DATA_STRUCTURES.md`. `newContainerIDs`, profile markers.
- [iamfitsum/arc-export-windows](https://github.com/iamfitsum/arc-export-windows) at `e4e1bc0` (2025-08-04): `README.md`, `main.py`. Windows package path; JSON repairs.
- Zen has no built-in Arc importer ([zen-browser/desktop discussion #8967](https://github.com/zen-browser/desktop/discussions/8967)); arc2zen is the community route.
- Arc help center: [Arc Notes phase-out and export](https://resources.arc.net/hc/en-us/articles/19233788518039), [Little Arc](https://resources.arc.net/hc/en-us/articles/19235387524503-Little-Arc-Quick-Lookups-Instant-Triaging), [Split View](https://resources.arc.net/hc/en-us/articles/19335393146775-Split-View-View-Multiple-Tabs-at-Once), [Archive](https://resources.arc.net/hc/en-us/articles/19228855311127).
