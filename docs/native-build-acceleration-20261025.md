# Native build acceleration for the Flecha Chromium 158 Linux build

Research date: 2026-10-25. Scope: research only. No tool was installed, no GN argument or compiler flag was changed, no build was started or resumed, and the owner's browser was not touched.

## Recommendation

1. **Resume the interrupted recovery with the same command and do not change anything first.** From `/home/timpan4/.t3/builds/flecha-linux-158.nHjUam/src`, run `env -u __PYVENV_LAUNCHER__ third_party/depot_tools/autoninja -C out/Dev chrome`. Ninja skips the 20,977 actions recorded in the new `.ninja_log`. Every option below changes compile command lines, the build tool, or the output directory, so each one would rerun some or all of the completed work. None of them can speed up the remaining 36,257 cold actions on this machine.
2. **Do not run `gn gen`, `gn clean`, system `ninja`, or edit `out/Dev/args.gn` before the recovery finishes.** Afterwards, verify that the same wrapper reports no work, as the handoff already requires.
3. **For future warm builds, the main gain is keeping Ninja's incremental state intact.** Losing `.ninja_log` is what turned an edit into a full rebuild. Copying `.ninja_log` and `.ninja_deps` (about 123 MB together) before any risky build-tool operation would have prevented this incident.
4. **Do not adopt sccache for this configuration.** sccache 0.19.0 refuses to cache any compile that has `-fmodules`, and this build passes `-fmodules` to almost every C++ target.
5. **Consider Siso, and possibly ccache, only at the next planned cold build**, such as the next Chromium milestone rebase into a fresh output directory. Measure the result there. Do not convert `out/Dev`.
6. **Keep Chromium's pinned clang.** A different compiler invalidates every object, removes Chromium's clang plugins, and has no documented speed benefit. The installed system clang 23.1.4 is also older than the pinned clang 24 trunk.

## Current state (observed facts)

| Item | Observation | Source |
|---|---|---|
| Chromium | 158.0.8119.0, from the release tarball (`chromium-158.0.8119.0.tar.xz`). There is no `.gclient` or `.git` in or above `src`. | `src/chrome/VERSION`, build directory listing |
| Compiler | `third_party/llvm-build/Release+Asserts/bin/clang++`, clang 24.0.0git `e063be79adf127cce301fe5948273102a33012b6` (`CLANG_REVISION = 'llvmorg-24-init-14453-ge063be79'`) | `clang --version`, `tools/clang/scripts/update.py` |
| GN args | `is_debug=false`, `is_component_build=true`, all three symbol levels `0`, `use_remoteexec=false`, x64 Linux, no `cc_wrapper` | `out/Dev/args.gn` (mtime 13:56) |
| Clang modules | Active. `use_clang_modules` defaults to true here. 10,901 of the 10,928 target `.ninja` files that have `cflags_cc` contain `-fmodules`, plus `-fmodule-map-file=` and `-fmodule-file=` | `build/config/c++/c++.gni:93`, grep of `out/Dev/obj/**/*.ninja` |
| Clang plugins | The compile flags include `-Xclang -add-plugin` with `raw-ptr-plugin`, `find-bad-constructs` and `unsafe-buffers` | `out/Dev/obj/base/base.ninja` |
| Build tool | Pinned Ninja 1.12.1 (`version:3@1.12.1.chromium.4`). Pinned Siso is also present: `third_party/siso/cipd/siso` reports `siso v1.6.6` (`README.chromium` still says v1.5) | `DEPS:285,589`, `siso version` |
| Why Ninja, not Siso | `build/config/siso/.sisoenv` exists and `build_with_chromium = true`, so Siso would be the default. `autoninja` falls back to Ninja only because `out/Dev/.ninja_deps` exists. Switching this directory to Siso makes `autoninja` stop with "Run gn clean before switching from ninja to siso" | `build/toolchain/siso.gni`, `third_party/depot_tools/autoninja.py:160-195,498-510` |
| Job count | `autoninja` without remote execution uses `-j` = CPU count + `NINJA_CORE_ADDITION` (default 2), which gives 18 on 16 CPUs | `autoninja.py:596-602` |
| Original cold build | One uninterrupted run of 57,234 actions. Log created 14:01:04 and last written 15:18:58, about 78 minutes | `build-native.log` timestamps |
| Recovery run | 20,977 of 57,234 actions between 18:56:46 and 19:10:02, about 13 minutes. The log has no `FAILED` line. `.ninja_log` is a v6 log with 50,002 output entries, last written 19:10:02 | `rebuild-native-recovery.log`, `out/Dev/.ninja_log` |
| Action mix so far | 14,353 CXX, 2,810 ACTION, 2,797 CC, 309 SOLINK, 237 AR, 235 RUST, 127 ASM, 17 LINK | recovery log |
| Host | 16 CPUs. 30 GiB RAM, 18 GiB available when sampled. 64 GiB swapfile plus 30.5 GiB zram. 189 GiB free on `/home`. `out/Dev` is 6.0 GB. The kernel journal is readable and has no OOM lines for 18:50 to 19:15 | `nproc`, `free`, `swapon`, `df`, `du`, `journalctl -k` |
| Build processes | No `ninja` or `autoninja` process was running during this research | `pgrep` |
| sccache | `/usr/bin/sccache` 0.19.0 is installed (pacman: installed 2026-10-25 15:49, "Explicitly installed"). It is not wired into this build | `sccache --version`, `pacman -Qi` |

**Inference about the remaining time:** the first 37% of actions took about 17% of the original wall time. The remaining actions contain most of the Blink, V8 and `chrome` link work, so extrapolating from action counts underestimates the remaining time. No per-action timing exists from the original run because its `.ninja_log` was lost, so the remaining time is unmeasured.

## Options assessed

### Current interrupted cold rebuild

| Option | Effect on the 20,977 completed actions | Would it speed up the remaining 36,257? |
|---|---|---|
| Resume unchanged (recommended) | Kept | This is the baseline |
| Add `cc_wrapper` (ccache or sccache) | All C/C++ command lines change, so all of them rerun | No. The cache starts empty, so every compile is a miss plus cache overhead |
| Switch to Siso | Requires `gn clean`, which discards the 6.0 GB of outputs | No. This becomes a full cold build |
| Remote execution | Requires `use_remoteexec=true` with Siso, which requires `gn clean` | Only with a backend that has workers beyond this machine, and none exists |
| Different compiler | Every compile reruns | No documented benefit |
| tmpfs `out/` | Needs root and moving or remounting `out/Dev` (live-host change). A 6.0 GB output on 30 GiB of RAM competes with 18 compile jobs | Not recommended. Chromium's only published number comes from a 12 GB HP Z600 debug build, which does not apply here |
| Changing `-j` | No effect on completed work | Unmeasured. The recovery log has no failures and the journal has no OOM, so there is no evidence that the default is wrong. Do not tune it without a measured problem |

### Future warm builds

**Ninja incremental (keep).** Ninja rebuilds an output when its command line is missing from `.ninja_log` or has changed. Ninja does this by design, and it is why the lost log forced a full rebuild. Edits to Flecha patches recompile only the affected translation units unless they touch widely included headers. Preserve the log and always use the depot_tools wrapper, never system Ninja 1.13.2. Current upstream Ninja is v1.13.2 ([releases](https://github.com/ninja-build/ninja/releases)). Chromium pins 1.12.1, and the version mismatch caused the incident.

**Siso (adopt at the next fresh output directory).**
- Facts: Chromium's Linux instructions now say "Chromium uses Siso as its main build tool" ([build_instructions.md](https://chromium.googlesource.com/chromium/src/+/main/docs/linux/build_instructions.md)). Siso describes itself as "a drop-in replacement for Ninja" ([pkg.go.dev](https://pkg.go.dev/go.chromium.org/build/siso)), where v1.6.6 is listed on 2026-10-16. That matches this tree's pin. Without remote execution, `autoninja` runs `siso ninja --offline` and maps `-j` to `--local_jobs` ([siso_tips.md](https://chromium.googlesource.com/chromium/src/+/main/docs/siso_tips.md)). Siso is stricter than Ninja: it fails on missing restat outputs, missing inputs and depfile parse errors. It also locks the build directory against concurrent runs, which would have blocked the concurrent-run class of mistakes ([ninja_diff.md](https://chromium.googlesource.com/build/+/refs/heads/main/siso/docs/ninja_diff.md)).
- Fact: the pinned binary's help lists `-local_cache_enable` (default off) and `-cache_dir` (default `~/.cache/siso`). Chromium's docs do not document a local-only cache workflow, so its status for local builds is unverified.
- Inference: Chromium's docs justify Siso by remote execution. No primary source documents a local-only speedup over Ninja, so expect parity, not a gain, until it is measured. The practical reasons to switch are that it is upstream's default path and that it has the stricter correctness checks.
- Cost: a full cold build in a new output directory, about 6 GB more disk, and Siso's stricter failure modes against Flecha's patches.

**ccache (optional, only after a trial).**
- Facts: ccache 4.14.2 is current, released 2026-10-18 ([releases](https://github.com/ccache/ccache/releases)). Chromium supports `cc_wrapper = "ccache"` (`build/toolchain/cc_wrapper.gni`), but its Linux docs say ccache is "not useful if you're using Siso". ccache supports `-fmodules` only with both direct mode and depend mode plus `sloppiness = modules`, and warns that this "May not detect changes in module internal state, or in implicitly built modules" ([manual 4.14.2](https://ccache.dev/manual/latest.html)). Compiler plugins are added to the hash.
- Inference: with Ninja incremental working, ccache helps only when identical compiles happen again. Examples are the lost-log recovery from this incident, a second output directory, re-extracting the same tarball, or going back to an earlier patch state. It does little across Chromium milestone bumps, because the pinned clang revision and most headers change. Whether Chromium's explicit-module compiles hit the cache is unverified.
- Cost: one full cold build to wire it in and populate the cache, cache disk space (the size is an owner choice, and none is proposed here), and a module-sloppiness correctness tradeoff. It cannot be combined with `use_remoteexec`.

**sccache (do not adopt here).**
- Fact: sccache v0.19.0 was released 2026-10-24 ([releases](https://github.com/mozilla/sccache/releases)). Its source marks `-fmodules` as `TooHardFlag`, which yields `CannotCache` ([`src/compiler/gcc.rs` at v0.19.0](https://github.com/mozilla/sccache/blob/v0.19.0/src/compiler/gcc.rs), line 230, with a test at line 2676; [`clang.rs`](https://github.com/mozilla/sccache/blob/v0.19.0/src/compiler/clang.rs), test at line 827).
- Inference: almost no C++ compile in this build would be cached. Making it useful would require `use_clang_modules=false`, which departs from Chromium's default configuration and has an unmeasured effect on compile time.

**Remote or distributed execution (only with an existing backend).**
- Facts: Chromium's Google RBE for external contributors requires an invitation for contributors with try-job access and is "a best-effort process". Any REAPI-compatible backend also works ([server list](https://github.com/bazelbuild/remote-apis#servers)). It needs `.gclient` `custom_vars` (`reapi_address`, `reapi_instance`, `reapi_backend_config_path`), `gclient sync`, a custom `backend.star`, authentication, and `use_remoteexec = true` with `use_siso = true` ([build_instructions.md](https://chromium.googlesource.com/chromium/src/+/main/docs/linux/build_instructions.md)). Icecc requires `is_clang=false`, which means GCC. GCC is community-supported only ([toolchain_support.md](https://chromium.googlesource.com/chromium/src/+/main/docs/toolchain_support.md)) and turns off this build's clang modules and plugins.
- Inference: this checkout came from a tarball and has no `.gclient`, so the documented `gclient sync` setup does not apply as written. A REAPI server running only on this workstation adds a cache but no CPU. A speedup needs additional worker machines. That is a large infrastructure decision for the owner, not a tuning step.

### Changing compiler versions

- Facts: Chromium uses "upstream clang built at a known-good revision that we bump every two weeks or so", and clang "is the only supported compiler". A custom clang needs an absolute `clang_base_path` and disabling Chromium's plugins (`clang_use_chrome_plugins=false`) ([clang.md](https://chromium.googlesource.com/chromium/src/+/main/docs/clang.md)). Chromium aims to build with its pinned clang and clang trunk. Generally only the most recent LLVM is supported ([toolchain_support.md](https://chromium.googlesource.com/chromium/src/+/main/docs/toolchain_support.md)). The latest LLVM release is 23.1.4 (2026-10-20, [releases](https://github.com/llvm/llvm-project/releases)). The installed system clang is also 23.1.4, while the pin is 24 trunk.
- Inference: switching compilers rebuilds everything and loses the raw_ptr and unsafe-buffers plugin checks. It also risks failures with the newer warning and module flags that the pinned clang accepts, and has no documented speed benefit. Chromium's pinned toolchain update replaces the compiler with each Chromium version anyway.

## What invalidates existing outputs

- Any change to GN arguments that changes compile flags, including `cc_wrapper`, `use_clang_modules`, `clang_base_path`, `use_remoteexec` and symbol levels. Ninja reruns every action whose command changed.
- `gn clean`, which Siso requires before it takes over an output directory that Ninja has used.
- Running a different Ninja version that rewrites or removes `.ninja_log`. This is the cause of the current incident.
- Replacing the pinned clang, or updating the Chromium tarball. Both change compiler identity, sources and headers.

Environment-only changes that leave command lines unchanged, such as ccache variables when no `cc_wrapper` is set, have no effect on this build.

## Next action for this checkout

1. The owner or parent agent first checks that no build is running and that `out/Dev/args.gn` and `build.ninja` are unchanged (mtimes 13:56 and 14:00). Then they resume with the exact recorded wrapper command from `src/`, appending to the recovery log.
2. After it passes, run the same wrapper again to confirm "no work to do", then rerun `chromium/tests/native-workflows.mjs` as the handoff describes.
3. Before any later risky build-tool operation, copy `out/Dev/.ninja_log` and `out/Dev/.ninja_deps`.
4. At the next planned cold build, choose whether to start a fresh Siso output directory. Optionally run a measured ccache trial there, reading ccache's "Could not use modules" and hit counters. That trial needs an owner decision on disk and cache size, and on accepting `sloppiness = modules`.

## Verification limits

- No speedup was measured for any option. All performance statements are documented behavior or labelled inference.
- Build durations come from log-file creation and modification times, not from internal timing. Both logs were checked to start with "Entering directory" and to cover one invocation each.
- ccache's behaviour with Chromium's explicit Clang modules, Siso's `-local_cache_enable`, and Siso's local-only performance relative to Ninja were not tested.
- Google RBE eligibility for this fork was not checked.
- Upstream doc pages were read at `main` on 2026-10-25. The local tree's docs at 158.0.8119.0 match on the points cited (Siso as main tool, ccache and Siso note, remote-execution setup).
