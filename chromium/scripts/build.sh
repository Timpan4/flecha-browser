#!/bin/bash
# Puts Flecha's branding and patches on the Chromium checkout, then builds the
# dev (component) app at out/Dev/Flecha.app. Extra arguments go to autoninja.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
src="${CHROMIUM_SRC:-/Volumes/EXT_FLASH_1TB/projs/flecha/chromium/src}"
export PATH="${DEPOT_TOOLS:-/Volumes/EXT_FLASH_1TB/projs/flecha/depot_tools}:$PATH"

"$here/scripts/apply.sh"
cd "$src"
autoninja -C out/Dev chrome "$@"
