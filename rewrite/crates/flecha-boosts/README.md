# flecha-boosts

`flecha-boosts` holds Boosts, Flecha's per-site user CSS and TypeScript. It ports the Chromium shell's `lib/boostFiles.ts`, `features/boosts.ts` and `features/compileBoost.ts`. It depends on flecha-core but not on CEF or GPUI.

## Names and matching

A Boost is named after its host: `github.com.ts` (script), `github.com.css` (styles), or `all.css` and `all.ts` for every site.

- `BoostName::parse` follows `parseBoostName`. A stored host may hold any text except a JavaScript line terminator.
- `boost_matches` follows `boostMatches`:
  - `all` takes any URL that starts with `http:` or `https:`.
  - Any other host must equal the URL's WHATWG hostname, or the hostname must end in `.<host>`.
  - Ports and user info play no part. IDN hosts compare in punycode.
  - Under `boostMatches`, a stored uppercase or Unicode host never matches, and a trailing-dot hostname (`github.com.`) doesn't match `github.com`.
- `applies` is `boost_matches` on http and https pages only. Styles, reloads and build-error reports use it. This is a deliberate deviation from the shell. `boostMatches` takes any scheme with a host, such as `chrome://settings` for `settings.css`. The shell was a component extension with `<all_urls>` (patch 0021), so Chromium let it style and script `chrome://` and extension pages. It had no `file:` access, but `chrome.tabs.reload` still reloaded matching `file:` tabs. The rewrite's internal pages are GPUI screens, so it applies Boosts to web pages only.
- `user_script_matches` follows the user scripts the shell registered with Chromium: `*://<host>/*` and `*://*.<host>/*`. Chromium's `URLPattern` (`extensions/common/url_pattern.cc`) differs from `boostMatches` in three ways:
  - It lowercases and punycodes the pattern host.
  - It trims trailing dots from both hosts.
  - It doesn't match subdomains of an IP address.

  A script Boost ran where either `applies` or this matched. Chromium registers user scripts all or nothing (`user_scripts_api.cc`), so one host it rejects turns this path off for every script.
- `boost_host_for` follows `boostHostFor`. It names a new Boost after the page's hostname without `www.`.

## Storing

`Boosts` keeps every Boost in the order it was first saved, because Boosts apply in that order. `list` sorts by name in UTF-16 order, as JavaScript does.

| Method | Shell function | Notes |
|---|---|---|
| `create` | `create` | Trims and lowercases the host and drops `www.`. The host must match `^[a-z0-9-]+(\.[a-z0-9-]+)*$`. A new Boost gets the shell's template. |
| `file_for` | `fileFor` | `create` for a page's host. |
| `save` | `save` | The host must match `^[a-z0-9.-]+$` (any case). A script stores the editor's build, or an empty build when there is none. |
| `remove` | `remove` | |

Each write returns an `Effect`:

- `Restyle`: restyle open pages of the host at once.
- `Reload`: reload them.
- `Saved`: store the change but leave pages alone.
- `Unchanged`: nothing to store.

Incognito windows apply Boosts but write none, so the app must not call the writers there.

`snapshot` is the rewrite's Boosts file. It follows `flecha_core::storage`:

- It writes a versioned JSON snapshot. The app keeps a current copy and a last-good copy.
- A damaged current snapshot is handed back in a `Damaged` write before the last-good copy replaces it.
- A snapshot from a newer schema is refused, so an older Flecha can't overwrite it. `decode` reads the version first with `flecha_core::storage::read_schema_version`, so a newer file is refused whatever else it holds, even JSON this version can't parse.

`Boosts::from_shell` reads the shell's `chrome.storage.local` values: `boosts` (name to code) and `boostsCompiled` (name to build). It keeps their key order. It reports keys the shell never used, builds it had to drop, and either value when it isn't an object. The shell read such a value as holding no Boost or no build and kept using the other key, and so does the import. The profile migration owns reading the old profile.

The shell has no Boost import or export. It sets no size limit on a Boost.

## Injection

| | Styles | Scripts |
|---|---|---|
| Method | `styles_for(frame, url)` | `scripts_for(frame, url)` |
| When | `RunAt::DocumentStart`, when the navigation commits | `RunAt::DocumentEnd`, once the DOM is ready |
| Frames | Main frame only | Main frame only |
| How | User style sheet (`origin: 'USER'`) | Classic script in the page's own world, one script per Boost |

When saved styles change, the app removes the styles it inserted before and inserts `styles_for` again. A script whose last save didn't compile gives `ScriptInjection::ReportError`, a `[Flecha boost <name>] <error>` line for the page's console.

## Compiling

The shell compiles a script Boost on the Boosts page with sucrase (`typescript` and `imports` transforms). It wraps the output in a function, and wraps that in a once-per-document guard. This crate ports both wrappers byte for byte:

- `compile_with` is `compileBoost`, given the transform as a function.
- `once_per_document` is the guard.

The TypeScript transform stays with the caller. Oxc has no ESM-to-CommonJS transform, so no Rust crate reproduces sucrase's output or error text. The shell's design keeps the compiler on the Boosts page, and its background only stores the build it is sent. `save` takes that build the same way.

### Limits of the wrapper

The shell's wrapper keeps a Boost's declarations out of the page's global scope. It doesn't contain the Boost's own text, and the rewrite keeps that behaviour:

- **HTML-like comments.** sucrase parses a module, where `<!--` is an operator. The page runs a classic script, where Annex B reads `<!--` as a line comment. A Boost written that way can close the wrapper function early, and the once-per-document guard too. `tests/parity.rs` records both cases.
- **Shared markers.** The guard's marker is a registered symbol on the page's global object. The page and every Boost in the main world can read it, or set it first to stop a Boost from running.

Each Boost runs as its own script, so one Boost's text can't change how another Boost parses.

## Tests

`tests/parity.rs` replays `tests/fixtures/boosts-parity.jsonl`. On non-web pages it checks the fixture against a model of the shell and the crate against its web-only rule, as the named deviation `web_only_deviation`. `scripts/generate-boosts-fixture.ts` records that fixture by running the shell's own modules under Node against a mocked `chrome` API. The fixture covers:

- names, hosts and matching, including the user script patterns the shell registered;
- the shell's calls to store, register, execute, style and reload, including on `chrome://`, extension and `file:` tabs, where the mock refuses scripting as Chromium did on `file:`;
- stored values that aren't objects;
- sucrase's output for hostile Boosts, and what the wrapped Boost does when it runs twice in one Node `vm` context.

Node can't run Chromium's `URLPattern`. The fixture records the patterns the shell registered, and `registered_user_scripts_follow_chromium` checks the matcher against cases worked out from `url_pattern.cc`.

CI regenerates the fixture and fails on drift. Regenerate it from the repository root:

```sh
node rewrite/crates/flecha-boosts/scripts/generate-boosts-fixture.ts
```

`tests/storage.rs` covers the snapshot, the damaged-file fallback, newer files the decoder must refuse unread, the shell's stored shape including malformed values, and the web-only scheme rule.
