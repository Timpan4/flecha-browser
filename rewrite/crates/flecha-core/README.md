# flecha-core

`flecha-core` owns durable tab, Space, folder and window IDs, sidebar state, tab placement, selection and recency, split state, archive and reopen rules, and settings. It applies Today bulk-close rules, site ad-block exceptions, shortcut conflicts, command-bar parsing, and tab sleep policy. The application supplies IDs and clocks.

## Storage

`storage::encode` writes schema version 3. `storage::migrate_legacy` reads the saved JSON value from a copy or export of a Chromium profile. It accepts versions 1 and 2 and unversioned snapshots. Version 1 moves each Space's Today tabs into a single window. Version 2 carries over each window's own Today tabs, shared pins and Favorites, and recently closed windows.

`storage::load` returns repaired state plus the writes needed to keep damaged and last-good copies. Apply recovery writes in order and call `LoadedState::started` after startup succeeds. A newer current schema blocks recovery. A newer last-good copy stays available to a newer reader, and a damaged current value is still set aside. Repairs assign distinct IDs to duplicate Spaces and folders while retaining their contents. Version 2 window memberships keyed by a duplicate Space ID stay with the first Space. The application owns the storage path, atomic writes, and error reporting.

`import_spaces` merges or appends parsed Spaces, folders, favorites, splits, and archive entries in one transaction. `storage::set_aside_before_import` prepares the pre-import snapshot. Persist it before applying the import, and read it with `storage::state_before_import`.

## Engine and UI

CEF owns browser handles, cookie stores, page-state restoration, media state, and applying sleep decisions. Migration retains navigation URLs and titles. Chromium's opaque `pageState` and the old agent bridge token are dropped because the new engine and services own those formats and credentials.

GPUI owns command-bar ranking across sources, Peeks, shortcut registration, local date labels, and general layout undo. Browser discovery, file parsing, and visit-history import use services and engine APIs. Import previews, stale-preview checks, and result text depend on the source and UI flow. Suggestion scheduling, cancellation, character-set decoding, and cookie policy belong to the HTTP client. The core tracks responsive mirrors as Today tabs. The engine creates and operates the mirrored browser and follows the source's navigation.

## Differences from the Chromium shell

- Shortcut comparisons use the native Chromium accelerator parser's rules. Unsupported saved keys fall back to defaults. Editing a shortcut unbinds every command configured to take the same key, including a shadowed default.
- Evicted windows retain their original close time in the archive. Reopening a window whose active Space lost its Today tabs opens the first Space that retained tabs.
- Suggestion requests keep JWTs in pasted prose local. GitHub issue shortcuts require a complete repository segment and fall back to GitHub search for ambiguous paths.
