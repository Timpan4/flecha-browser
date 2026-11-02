# Nightly native build reuse

Nightlies search earlier default-branch runs of `nightly-release.yml` for a compatible Chromium tree. Missing trees, failed lookups or transfers, failed extractions and invalid restored state fall back to clean preparation. Failed restoration or validation removes only the work directory created for that attempt; unexpected pre-existing trees are preserved and rejected. Failed and cancelled runs can supply checkpoints; pull requests, forks and other branches cannot.

Only `gh` subprocesses inherit the GitHub token. Source preparation, native build, Git fingerprinting and archive subprocesses receive an environment without GitHub tokens.

The SHA-256 compatibility key includes tracked Chromium revision, patches, branding, build configuration and Windows SDK setup; effective GN arguments with a normalized run number; depot_tools revision; source preparation code; platform and absolute workspace path; hosted runner image OS and version; and the selected Xcode or Visual Studio version. Changing a native input or runner image starts a clean build. Shell and packaging changes can reuse compiler outputs.

If hosted image metadata is unavailable, the key includes the current run ID to prevent cross-run reuse.

After restoration, the script checks the previous source SHA, compatibility key, Chromium revision, platform and depot_tools revision. It replaces the staged shell, regenerates GN arguments with the current version and run number, and clears package completion. Ninja then checks the retained native outputs before current-source packaging runs. Package provenance records the current commit and the reused run, artifact and source commit.

Incomplete stages upload their usual checkpoint, then a small `reuse-<platform>-<key>-<stage>` index. Discovery ignores indexes whose checkpoints have expired or been superseded. Completed native builds save `native-<platform>-<key>` before packaging. That full tree uploads even when packaging fails. Later stages still use state-only completion markers.

Native snapshot creation, native artifact upload and reuse-index upload are optional. Their failure does not block packaging or the required package/checkpoint uploads. Cancellation still propagates.

Artifacts use the existing 35-day workflow retention policy. Older runs without compatibility indexes cannot seed reuse. The first compatible run must build the tree, and native changes still require compilation. Reuse adds full-tree compression and transfer after native compilation; it does not eliminate checkpoint overhead. The focused tests cover compatibility, trusted-run selection, archive restoration, shell replacement and provenance renewal. Full Chromium reuse and its speedup require a hosted nightly measurement.
