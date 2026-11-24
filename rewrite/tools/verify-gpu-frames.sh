#!/usr/bin/env bash
# Page frames on a GPU, under the isolated launcher's headless weston (Vulkan).
# Usage: tools/verify-gpu-frames.sh [frames|fallback]
# `fallback` needs a debug build: it rejects the first shared texture on purpose.
set -euo pipefail

exec "$(dirname -- "${BASH_SOURCE[0]}")/isolated-run.sh" wayland -- \
  node tools/verify-gpu-frames.mjs "${1:-frames}"
