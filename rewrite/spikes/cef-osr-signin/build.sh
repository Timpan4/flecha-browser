#!/usr/bin/env bash
# Builds the spike. The workspace's cef-rs is the source Spike 1's prepare.sh stages and
# patches under target/spike1-src; its build script links against Spike 1's staged copy of the
# pinned beta archive and checks it against rewrite/pins.toml. The runtime this spike loads is
# the one its own prepare.sh staged (run.sh). Both prepare steps must have run.
# Arguments go to `cargo build`.
set -euo pipefail
# shellcheck source=scripts/cef-pins.sh
. "$(dirname -- "${BASH_SOURCE[0]}")/scripts/cef-pins.sh"
check_runtime
if [[ ! -f $rewrite/target/spike1-src/cef-rs/.flecha-revision ]]; then
    printf 'No staged cef-rs source at %s. Run spikes/gpui-cef-compositor/prepare.sh first.\n' \
        "$rewrite/target/spike1-src/cef-rs" >&2
    exit 1
fi
cd "$rewrite"
exec cargo build --locked -p cef-osr-signin "$@"
