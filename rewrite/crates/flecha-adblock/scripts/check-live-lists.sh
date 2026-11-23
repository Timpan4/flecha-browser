#!/usr/bin/env bash
# Opt-in: downloads EasyList and EasyPrivacy from their real URLs with the real transport, then
# measures a cold and a warm load of them, each in its own process. Tests never use the network.
# Usage, from rewrite/: bash crates/flecha-adblock/scripts/check-live-lists.sh [scratch-dir]
set -euo pipefail

cargo=${CARGO:-cargo}
dir=${1:-$(mktemp -d)}
mkdir -p "$dir"
"$cargo" build --release -p flecha-adblock --example fetch_lists --example measure
target=$("$cargo" metadata --format-version 1 --no-deps | jq -r .target_directory)
"$target/release/examples/fetch_lists" "$dir"
"$target/release/examples/measure" cold "$dir"
"$target/release/examples/measure" warm "$dir"
echo "lists and engine.dat: $dir"
