# Flecha importers

`detect_browsers` and `read_browser` take application-supplied platform paths. `read_bookmarks_html_file` and `FlechaExport::read` take a selected file. `ImportPlan::preview` and `ImportPlan::apply` cover the browser import workflow. `apply` sends Space changes through `flecha-core` and passes history visits to the application's history writer. `FlechaExport::apply` requires the caller to save the before-import snapshot successfully before it replaces state.

Run the TypeScript importers against the committed bookmarks HTML, Arc fixture, and synthetic profiles from the repository root.

```sh
node rewrite/crates/flecha-import/scripts/generate-import-fixture.ts
```

`tests/fixtures/import-parity.jsonl` records TypeScript discovery, raw plans, previews and applied state. Each case with a different Rust expectation carries its reason and the full expected output. `cargo test --locked -p flecha-import` compares the readers and apply paths against that fixture. CI also checks that regeneration leaves the fixture unchanged.

## Deliberate differences

| Boundary | Rust behavior and reason |
| --- | --- |
| File access | Browser discovery checks metadata. Reads under a supplied root use directory handles, reject symlinks and special files, and open allowlisted files only. The TypeScript readers follow file symlinks. A link could redirect a read into unrelated browser or user data. |
| SQLite | The database and available WAL are copied through validated handles into a private temporary directory. Firefox bookmarks and history use the same copy. The reader opens the copy read-only and authorizes only expected table columns, preventing a forged schema from exposing data through views or virtual tables. A failed WAL copy is discarded and reported. |
| Limits | Browser text reads use Node's signed 32-bit `fs.readFile` boundary. Browser JSON validation and walking use explicit stacks. The reader keeps 405 nested JSON containers, enough for 200 Chromium bookmark folders plus the document and leaf wrappers. The folder count comes from Chromium's `base/json/json_common.h` depth of 200, with two JSON containers per folder. Deeper subtrees are skipped with a warning and their siblings are retained. Safari uses the shipping 64 MiB `plutil` output limit for its input and expanded XML and the prior serde_json nesting boundary of 127 collections. The shipping 2,000-bookmark, eight-folder-level, and 5,000-history-row limits still apply where the TypeScript reader uses them. |
| Strings | A lone surrogate in JSON or a numeric HTML entity becomes U+FFFD. Valid pairs are preserved. HTML text and import name matching use the shared ECMAScript whitespace rules. Imported titles and names keep their full length, as in the shipping import path. |
| Damaged records | A malformed Chromium bookmarks file leaves the other bookmarks file available. A valid JSON number beyond the finite range becomes null with a warning. JavaScript represents it as Infinity. The repair preserves the rest of the document and lets each field's existing fallback or skip rule apply. Counts report malformed bookmark, history and Arc records where TypeScript silently skips, coerces or stops. Non-text history URLs and Arc archive times outside the core timestamp range are dropped. |
| Firefox folder IDs | The first reached folder owns the children for its ID, in toolbar, menu, unsorted and mobile traversal order. Later references and cycles are counted as malformed. TypeScript expands duplicate folder IDs again, so a small set of duplicated rows can produce an exponential tree. |
| Firefox place IDs | Rust reads SQLite place rows once in the table's scan order and keeps the first row for each ID. Integer IDs, integral real values within the signed 64-bit range, and text that parses as an integer use the same key. Other values match by type and value. Null IDs have no match. Later rows with the same key are dropped and counted in a warning. TypeScript joins all matches and can produce a quadratic number of rows. |
| Arc shared items | The first reached sidebar item owns its contents. Rust visits each Space's pinned and unpinned roots in order, then favorites. A later reference keeps its Space but does not duplicate the contents. The reader reports how many shared references were skipped. TypeScript expands the same container again for each Space. |
| Arc identifiers | Item IDs and references preserve lone UTF-16 code units, as in TypeScript. IDs that Rust cannot represent as Unicode use a reserved internal encoding. U+FFFD repair still applies to titles and URLs. |
| Arc nested splits | Split views are evaluated inside out. Each directly contained tab can be kept in one split. Overlapping outer splits are counted as skipped and their tabs stay in place. TypeScript records overlapping splits with separate copies of the same tab URLs. |
| Warnings | Diagnostic strings omit source paths, profile names, parser errors, and arbitrary Arc keys. Known Arc feature names are listed by name and other kinds are counted as `unknown`. An empty HTML import uses the generic file label. |
| Safari | The Rust plist library reads XML and binary bookmarks in process. The TypeScript helper converts the input with macOS `plutil`. |
| Apply | Core validates that web tabs and archive entries have a usable address. Whole-state imports use the core migration and repair rules and accept the current Rust snapshot schema as well as legacy exports. The caller owns the confirmation UI. |

The importer adds [rusqlite 0.43.0](https://github.com/rusqlite/rusqlite/releases/tag/v0.43.0), [cap-std and cap-fs-ext 4.0.3](https://github.com/sunfishcode/cap-std/releases/tag/v4.0.3), and [plist 1.10.4](https://github.com/ebarnard/rust-plist/releases/tag/v1.10.4). It reuses the workspace's regex 1.13.2, serde_json 1.0.154, indexmap 2.14.2, tempfile 3.27.0 and Unix rustix 1.1.6.
