# Free GitHub Actions runners for Chromium

Investigated on 2026-11-01. Free standard GitHub-hosted runners can build Chromium on Linux, Windows and macOS. Successful upstream runs demonstrate all three platforms. Flecha's pinned Chromium 158 and its patches have not been built on those runners, so these examples establish a workable approach, not a verified Flecha release pipeline.

## Pricing and capacity

[GitHub's runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners) says standard runners are free and unlimited for public repositories. Its advertised 14 GB storage figure is not a sound basis for concluding that cleanup cannot make a Chromium build fit. Installed tooling can be removed, and successful builds below demonstrate usable capacity beyond that figure. Reclaimed space depends on the runner image; this investigation did not measure a fresh runner.

[GitHub's limits](https://docs.github.com/en/actions/reference/limits) impose a six-hour execution limit per hosted job. A complete build can span sequential jobs by saving the unfinished source/output tree as an artifact and restoring it in the next job. Compilation must stop early enough to complete that handoff before the platform terminates the job. Exact compilation and handoff allowances need measurement for Flecha.

## Linux evidence

[Portable Linux run 41095740465](https://github.com/ungoogled-software/ungoogled-chromium-portablelinux/actions/runs/41095740465) succeeded on 2026-10-31. The jobs API reports `ubuntu-latest` for its compilation and packaging jobs. Published artifact names identify Chromium 156.0.8078.79. This is a full Chromium build precedent, not just shell compilation.

Its [workflow](https://github.com/ungoogled-software/ungoogled-chromium-portablelinux/blob/6f0aa178c2394fc0a177edc0fd64e41f402a0418/.github/workflows/build-steps.yml) chains resumable jobs. Its [cache import script](https://github.com/ungoogled-software/ungoogled-chromium-portablelinux/blob/6f0aa178c2394fc0a177edc0fd64e41f402a0418/.github/scripts/import-cache.sh) removes unused Android, Haskell, Java, Google Cloud, .NET and Swift tooling, restores the build tree, then removes the archive. The [export script](https://github.com/ungoogled-software/ungoogled-chromium-portablelinux/blob/6f0aa178c2394fc0a177edc0fd64e41f402a0418/.github/scripts/export-cache.sh) removes the download cache before compressing the tree.

## Windows evidence

[Windows x64 run 41098495420](https://github.com/ungoogled-software/ungoogled-chromium-windows/actions/runs/41098495420) succeeded on 2026-11-01. Its jobs API reports `windows-2022`. The [workflow](https://github.com/ungoogled-software/ungoogled-chromium-windows/blob/8492884eb30fdd9417ea7816aa92def805c97c74/.github/workflows/reusable-build.yml) chains build stages; the [stage action](https://github.com/ungoogled-software/ungoogled-chromium-windows/blob/8492884eb30fdd9417ea7816aa92def805c97c74/.github/actions/stage/index.js) archives the unfinished build on an intentional compilation timeout and restores it in the following stage.

The first compilation stage started 2026-10-30 at 17:24 UTC; the eighth stage finished 2026-11-01 at 04:54 UTC. This proves that free builds can take longer than a daily schedule interval. It does not predict Flecha's build duration. The final run contains both a finished `chromium` artifact and a `build-artifact` checkpoint, as reported by GitHub's artifacts API.

## macOS evidence

[macOS run 20473752190](https://github.com/ungoogled-software/ungoogled-chromium-macos/actions/runs/20473752190) succeeded on 2025-12-26. Its jobs API reports `macos-15` for Apple Silicon and `macos-15-intel` for Intel. This historical run predates the project's switch to sponsored Depot runners; the current workflow's use of Depot must not be mistaken for evidence that free macOS runners cannot build Chromium.

The [workflow at the successful source revision](https://github.com/ungoogled-software/ungoogled-chromium-macos/blob/9a2d4e1b319cbf30e30d68e507fedb1d01d7b7f9/.github/workflows/building.yml) hands unfinished builds between jobs and removes Android tooling. Its [Xcode preparation script](https://github.com/ungoogled-software/ungoogled-chromium-macos/blob/9a2d4e1b319cbf30e30d68e507fedb1d01d7b7f9/.github/scripts/github_prepare_xcode.sh) keeps the selected Xcode version, removes other Xcode installations and deletes simulator devices. Those are examples from disposable hosted VMs, not permission to clean the user's workstation. Their exact Xcode version must not be copied into a Chromium 158 workflow without checking compatibility.

## Implications for Flecha

Use free standard runners as the first implementation target. Paid or self-hosted runners are not a prerequisite established by this research.

1. Build resumably and preserve the prepared source, toolchains and output between compilation jobs. Track the Flecha commit, Chromium revision, patches, build arguments, architecture and toolchain so incompatible objects are not reused.
2. Measure free disk before and after targeted image cleanup and throughout checkout, compilation, checkpointing and packaging. Compressed and extracted trees can coexist, so compilation alone does not prove packaging will fit.
3. Put scheduled and manual runs in the same workflow-level concurrency group. A run that spans several jobs must retain the release lock across the entire workflow.
4. Keep the requested schedule around 02:00 UTC, but do not promise a completed release every day. A serialized build can still be running when the next nightly trigger arrives.
5. Publish a nightly prerelease only after all requested platforms build and package successfully. Validate Flecha 158 on the actual runners before claiming this release pipeline works.

## Current task state

Research is complete. The repository was inspected, upstream workflow code was read, and GitHub run/job/artifact metadata was checked. These findings support the implementation in `.github/workflows/nightly-release.yml`; they do not establish that Flecha's Chromium 158 build or packages work on hosted runners. The workflow's first actual run supplies that evidence. See the README for the implemented schedule, manual command, checkpoints and package formats.
