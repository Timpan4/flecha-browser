# flecha-adblock

`flecha-adblock` blocks ads and trackers with [adblock-rust](https://github.com/brave/adblock-rust), using the same EasyList and EasyPrivacy lists as the Chromium shell. It depends on flecha-core but not on CEF or GPUI.

## API for the CEF hook

- `AdBlocker::should_block(&Request)` decides whether to cancel a request. The `Request` carries the request URL, the URL of the frame that made it, the tab's top-level URL and the resource type. Third-party and `$domain=` options match against the frame. The per-site switch matches against the top-level page, because the Chromium shell's main-frame `allowAllRequests` rule covered every frame inside an allowed page.
- `AdBlocker::cosmetic_filters(page_url)` returns the page's hide selectors, its exceptions and whether generic hiding is off. `AdBlocker::hidden_class_id_selectors` returns generic class and id selectors for the classes and ids the page reports. `CosmeticFilters::stylesheet` writes one `display:none` rule per selector, as `cosmetic.css` did.
- `AdBlocker::set_enabled` follows Settings' "Block ads and trackers" (`Settings::ad_block`). `AdBlocker::set_allowed_sites` follows `Settings::ad_block_allowed`.

Every call reads one immutable snapshot. The read lock is held only to clone an `Arc`, and the updater holds the write lock only to swap a new snapshot in. Matching runs outside the lock, with no I/O, so the CEF IO thread never waits on a list update. Until the first engine is installed, nothing is blocked or hidden. Main-frame navigations are never blocked.

## Lists and updates

`Updater::spawn` starts the `flecha-adblock-updater` thread and returns at once. Loading and refreshing never run on GPUI's foreground thread or a CEF thread, and the updater uses no GPUI timer.

1. On launch, it installs `engine.dat` (adblock-rust's serialization) if its key matches the cached list text. Blocking starts without parsing, whatever the lists' age.
2. It then fetches what is due. The rules come from `chromium/shell/build-adblock.mjs`:
   - A cached list younger than four days (`MAX_AGE_MS`) is used as is. An older one, or none, is downloaded again.
   - A download sends `Accept-Encoding: gzip` and has 30 seconds, shared across up to 20 redirects (the fetch spec's limit). Redirects must stay on HTTPS.
   - The body must match Content-Length. It is decoded strictly from gzip, deflate or br, and must be valid UTF-8. It must start with `[Adblock Plus …]`, carry `! Title: <id>`, match its ABP checksum if it has one, and contain one network filter the engine keeps. An uncompressed body with no Content-Length needs a checksum.
   - If a download fails, a cached copy that passes the checks is used, whatever its age. Downloads are written to `<id>.txt` through a temporary file and a rename.
3. When the list text changed, it loads `engine.dat` for the new key, or parses the lists, installs the engine and saves `engine.dat`.
4. It sleeps until the oldest list turns four days old. After a failed download it sleeps for `UpdaterConfig::retry_after_failure`. build-adblock.mjs has no retry interval, so the app has to supply this value.

`Updater::refresh_now` downloads both lists at once. Each pass produces an `UpdateReport` with warnings, failures and lines left out, for the app's log.

The app owns the cache directory.

## adblock-rust

The crate uses adblock 0.13.6, the current release on crates.io (published 2026-11-17).

- License: MPL-2.0. The copyleft is per file, so linking it into this MIT crate is fine, but changes to adblock-rust's own files would have to be published under MPL-2.0.
- Features: the defaults are off, so `single-thread` is not enabled and `Engine` is `Send + Sync`. `embedded-domain-resolver` (the public suffix list, for third-party checks) and `full-regex-handling` are on.
- Resources: network blocking and element hiding need no resource file. Scriptlets (`##+js(...)`) and `$redirect` stubs do. Those resources come from uBlock Origin under GPL-3.0, so none are loaded. Redirect filters are left out, scriptlets inject nothing, and procedural filters are not returned.
- `engine.dat` (6.0 MB for the 2026-11-23 lists) is a cache. It holds a magic line, a SHA-256 key over `RECIPE` and each list's id, length and text, the page exception lines with a little-endian `u64` length and their SHA-256, then adblock-rust's serialization. adblock-rust's checksum covers only its own part, so the crate hashes the page exception lines itself. adblock-rust rejects a file written by another crate version or with a bad checksum. The crate also rejects one keyed to other list text, and the engine is then rebuilt from the text. Change `RECIPE` in `src/engine.rs` whenever `build_engine` changes what it keeps or the file layout changes.

## Load time and memory

Measured on 2026-11-23 on BIG-CHONKER-cachy (AMD Ryzen 7 9800X3D, Linux 7.3.5-2-cachyos, rustc 1.100.0), release build, with the lists downloaded that day (EasyList 2,053,258 bytes, EasyPrivacy 1,521,678 bytes). Each figure is three runs of `examples/measure.rs`, each run in its own process. Peak memory is `VmHWM` from `/proc/self/status`.

| Load | Time | Peak RSS |
| --- | --- | --- |
| Cold: `build_engine` parses both lists | 59–60 ms | 33.5–33.9 MB |
| Warm: `warm_start` reads both lists, hashes them and deserializes `engine.dat` | 3.7–4.1 ms | 20.6–20.7 MB |

The two timers cover different steps. The cold time leaves out reading the lists and saving `engine.dat`, but the cold peak includes the buffer that serialization allocates. The warm time includes reading and hashing the list text.

## Malformed input

One bad line or stored entry never fails a whole load.

- adblock-rust skips list lines it can't parse. `build_engine` reports each one with its list, line number and error, along with the redirect filters it leaves out. On 2026-11-23, 3,018 lines were left out. 2,981 didn't parse: 2,977 `$popup` filters, a `$csp` with a resource type, an empty `$redirect`, a `~document` and an HTML filter. The other 37 are `$redirect`, `$redirect-rule` and `$rewrite` filters. build-adblock.mjs dropped all of them too.
- Saved `adBlockAllowed` entries that aren't request-domain hostnames are dropped one by one. flecha-core counts them in `SnapshotRepairs::skipped_ad_block_sites`. `AdBlocker::new` and `set_allowed_sites` return how many they dropped.
- Hostnames compare as ASCII bytes. Page hosts come from the WHATWG URL parser (the `url` crate), as `new URL(url).hostname` did in the shell. Allowed entries must already be lowercase ASCII, so U+017F, U+212A and fullwidth forms are never folded into a match.

## Differences from the Chromium shell

The shell converted the lists to declarativeNetRequest. adblock-rust has no such limits, so it applies more of each list.

- Regular-expression filters apply. The shell left them out because Chromium caps them.
- There is no 30,000-rule budget and no 2,000-domain merging. Both were declarativeNetRequest limits.
- Site-specific hiding (`site##sel`) applies. An exception (`site#@#sel`) applies on its own site. The shell skipped site-specific hiding and dropped an excepted selector everywhere.
- Generic class and id selectors come from the classes and ids the page reports. The shell injected every generic selector into one stylesheet. Selectors with braces or unbalanced quotes, comments, brackets or parentheses are still left out.
- An `@@…$document` exception exempts the page or frame it matches and everything loaded inside it, as the shell's `allowAllRequests` rules did. adblock-rust lets any exception match a document request, so these lines are matched by a second engine built from them alone. The shell dropped a `$document` exception that also named resource types (`$document,subdocument`). This crate keeps it. Element hiding on an exempted page is unchanged, as in the shell. EasyList had one pure `$document` exception on 2026-11-23 (`optout.networkadvertising.org`).
- `$important` beats exceptions, as in uBlock Origin. The shell ignored `$important`, so an exception always won there. EasyPrivacy had 10 `$important` filters on 2026-11-23.
- `$elemhide` and `$ehide` aren't adblock-rust syntax, so those lines are reported and skipped. The shell read them as `$generichide`. EasyList and EasyPrivacy had none on 2026-11-23.
- A list counts as usable when adblock-rust parses one network filter it keeps, rather than the shell's converter.
- A saved allow list keeps its valid entries. The shell dropped the whole list for one bad entry, because declarativeNetRequest refuses the whole rule. Turning the switch off for a host that isn't a request domain (`news.example.`) is refused. The shell stored it, and lost the whole list on the next launch.
- Lists are refreshed at runtime, every four days per list, rather than at build time. A first launch with no network blocks nothing until a download succeeds.

## Tests

- `tests/lists.rs` and `tests/transport.rs` port `chromium/tests/build-adblock.test.mjs`. One test isn't ported: "reverse download completion preserves EasyList priority, rule IDs and the shared budget" covers declarativeNetRequest rule IDs and the rule budget, which don't exist here.
- `tests/sites.rs` ports `chromium/tests/ad-block-sites.test.mjs`. `site-ad-block-shell.test.mjs` covers which tab the GPUI site panel reloads, so it isn't ported.
- `tests/site_parity.rs` is the differential test. `scripts/generate-site-fixture.ts` runs the shell's `URL.hostname`, `isSiteHost`, `blocksAdsOn`, `withAdBlocking` and `decodeState` under Bun over a corpus of page URLs, raw hosts and saved lists. Regenerate it from the repository root with `bun rewrite/crates/flecha-adblock/scripts/generate-site-fixture.ts`. Network filter matching isn't compared there, because declarativeNetRequest runs only inside Chromium. `tests/engine.rs` covers it with fixture lists.
- Tests never use the network. `scripts/check-live-lists.sh` is opt-in. It downloads the real lists with the real transport into a scratch directory, then measures a cold and a warm load, each in its own process.
