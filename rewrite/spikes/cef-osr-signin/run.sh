#!/usr/bin/env bash
# Starts the spike with the runtime prepare.sh staged. The binary has no RUNPATH, so libcef.so
# is found only through LD_LIBRARY_PATH; main then checks the loaded runtime against the pin.
# Arguments go to cef-osr-signin. FLECHA_SPIKE_BIN overrides the binary (default target/debug).
set -euo pipefail
# shellcheck source=scripts/cef-pins.sh
. "$(dirname -- "${BASH_SOURCE[0]}")/scripts/cef-pins.sh"
check_runtime
bin=${FLECHA_SPIKE_BIN:-$rewrite/target/debug/cef-osr-signin}
if [[ ! -x $bin ]]; then
    printf 'No binary at %s. Build it with spikes/cef-osr-signin/build.sh.\n' "$bin" >&2
    exit 1
fi
export LD_LIBRARY_PATH="$cef_runtime${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$bin" "$@"
