# flecha-core

`flecha-core` owns durable tab, Space, folder and window IDs, sidebar state, tab placement, selection and recency, split state, archive and reopen rules, and settings. It applies Today bulk-close rules, site ad-block exceptions, shortcut conflicts, command-bar parsing, and tab sleep policy. The application supplies IDs and clocks.

## Storage

`storage::encode` writes schema version 3. It stores pinned trees as ordered records with depths so deep folders can save and restore without deeply nested JSON. The reader also accepts nested pin records. `storage::migrate_legacy` reads the saved JSON value from a copy or export of a Chromium profile. It accepts versions 1 and 2 and unversioned snapshots. Version 1 moves each Space's Today tabs into a single window. Version 2 carries over each window's own Today tabs, shared pins and Favorites, and recently closed windows.

Once a build ships, changing the snapshot format requires a schema version bump.

`storage::load` returns a startup handle. `LoadedState::ready` exposes repaired state and the current-slot writer when there are no pending writes. `LoadedState::apply_recovery` saves the damaged copy before restoring the current slot, stops on failure, and returns the ready state on success. Call `ReadyState::started` after startup succeeds. A newer current schema blocks recovery. A newer last-good copy stays available to a newer reader, and a damaged current value is still set aside. Repairs assign distinct IDs to duplicate Spaces and folders while retaining their contents. Version 2 window memberships keyed by a duplicate Space ID stay with the first Space. The application owns the storage path, atomic writes, and error reporting.

The reader validates JSON syntax and skips unknown fields throughout the snapshot without materializing their nested values. Legacy pinned folder children are part of the schema, so serde_json's normal nesting limit still applies to those trees. An overdeep legacy tree triggers recovery that preserves the original snapshot. Version 3 writes flat pin records.

`import_spaces` merges or appends parsed Spaces, folders, favorites, splits, and archive entries in one transaction. It preserves folder depth. Source readers can flatten their own input before calling it. Like the Arc reader, it accepts HTTP and HTTPS URLs, and reports rejected tab and archive entries.

For whole-state file imports, call `storage::set_aside_before_import` and persist the result before replacing the current state. Read the pre-import copy with `storage::state_before_import`.

## Engine and UI

CEF owns browser handles, cookie stores, page-state restoration, media state, and applying sleep decisions. Migration retains navigation URLs and titles. Chromium's opaque `pageState` and the old agent bridge token are dropped because the new engine and services own those formats and credentials.

State operations that return `TabChange` report their result and the durable tab IDs removed, including mirrors removed as a side effect. Closing a window reports its removed mirrors and tabs evicted from closed-window history. Its retained Today tabs stay available to reopen.

GPUI owns command-bar ranking across sources, Peeks, shortcut registration, local date labels, and general layout undo. Browser discovery, file parsing, and visit-history import use services and engine APIs. Import previews, stale-preview checks, and result text depend on the source and UI flow. Suggestion scheduling, cancellation, character-set decoding, and cookie policy belong to the HTTP client. The core tracks responsive mirrors as Today tabs. The engine creates and operates the mirrored browser and follows the source's navigation.

## Differences from the Chromium shell

- Shortcut comparisons use the native Chromium accelerator parser's rules. Unsupported saved keys fall back to defaults. Editing a shortcut unbinds every command configured to take the same key, including a shadowed default.
- When two saved keys resolve to one accelerator, Chromium's registration order picks the winner. The core displays no shortcut for the earlier binding when it loses.
- Exit Split selects the desktop source if its phone mirror was selected. This follows the responsive toggle and keeps the source visible after removing the mirror.
- Close Other Tabs and Close Tabs Below keep both responsive panes when invoked on either one, so closing the source cannot remove the anchor as a side effect.
- If another window claims the selected shared tab, the old window can select a local Today tab so it keeps a page visible.
- Auto-archive protects both panes of a selected split even in a background Space. Tabs with missing or invalid activity timestamps are ineligible for wall-clock expiry. Active-use sleep thresholds also require a known usage timestamp.
- The archive keeps the shell's front insertion and cap. At capacity, older tabs evicted from closed-window history can displace newer archive entries. Undo compares the original close times of all retained entries and windows. The Chromium shell compares the heads of those lists. Reopening a window whose active Space lost its Today tabs opens the first Space that retained tabs.
- Suggestion requests keep JWTs local even when pasted directly beside other text or with spaced, repeatedly encoded, or default-ignorable characters around or inside them. The local decoder accepts either standard Base64 or Base64URL for each segment, and accepts any JSON object header.
- Environment switches apply to HTTP and HTTPS pages. A blob URL stays on its original origin.
- The command bar treats malformed explicit HTTP and HTTPS input as invalid so its path, query, and fragment stay local.
- GitHub issue shortcuts require a complete repository segment and fall back to GitHub search for ambiguous paths.
