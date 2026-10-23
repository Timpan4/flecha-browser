#!/bin/bash
# Launches the dev build (out/Dev/Flecha.app) with its own profile and the
# Flecha shell extension loaded unpacked, so shell changes need no rebuild:
# rebuild the shell (node chromium/shell/build.mjs) and reload the window.
# Extra arguments go to Chromium, e.g. --flecha-show-top-chrome.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
. "$here/scripts/env.sh"
profile="${FLECHA_PROFILE:-$FLECHA_DISK/profiles/dev}"
shell_dist="$here/shell/dist"

if [ ! -d "$shell_dist" ]; then
  echo "note: $shell_dist does not exist yet; build the shell first" >&2
fi
mkdir -p "$profile"

# Newer Chromium ignores --load-extension unless this feature is off.
exec "$src/out/Dev/Flecha.app/Contents/MacOS/Flecha" \
  --user-data-dir="$profile" \
  --no-first-run \
  --disable-features=DisableLoadExtensionCommandLineSwitch \
  --load-extension="$shell_dist" \
  "$@"
