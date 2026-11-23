#!/usr/bin/env bash
# Builds the spike against the runtime prepare.sh staged. CEF_PATH is set only for this build,
# and the staged directory is checked first: when CEF_PATH is missing or invalid, cef-dll-sys
# silently downloads its default archive instead (sys/build.rs at the pinned rev).
# Arguments go to `cargo build`.
set -euo pipefail
# shellcheck source=scripts/cef-pins.sh
. "$(dirname -- "${BASH_SOURCE[0]}")/scripts/cef-pins.sh"
check_runtime
cd "$rewrite"
CEF_PATH=$cef_path exec cargo build --locked -p cef-osr-signin "$@"
