#!/usr/bin/env bash
# Shows that Clippy rejects control 1's mutant, a per-call GPUI timer armed with
# `executor.timer()` instead of `timers::Timers::arm`. Applies
# `direct-timer.patch` to a disposable copy of the committed tree
# (`git archive HEAD`), never to the caller's checkout, and runs workspace
# Clippy there with `-D warnings`, as CI does. Exits nonzero unless Clippy fails
# on that call with `clippy::disallowed_methods`. The copy is deleted on exit.
#
# Usage: clippy-guard-control.sh REWRITE_DIR
#   REWRITE_DIR  the rewrite/ dir of a checkout whose target/spike1-src was
#                staged by spikes/gpui-cef-compositor/prepare.sh
set -euo pipefail

usage() { echo "usage: $0 REWRITE_DIR" >&2; exit 2; }
(( $# == 1 )) || usage
[[ -d $1 ]] || { echo "not a directory: $1" >&2; usage; }
rewrite=$(cd "$1" && pwd)
here=$(cd "$(dirname "$0")" && pwd)
patch_file=$here/direct-timer.patch
[[ -f $rewrite/Cargo.toml && -f $patch_file ]] || { echo "need $rewrite/Cargo.toml and $patch_file" >&2; exit 2; }
[[ -d $rewrite/target/spike1-src ]] ||
    { echo "$rewrite/target/spike1-src is missing; run prepare.sh first" >&2; exit 2; }
: "${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set for the build lock}"
lock=$XDG_RUNTIME_DIR/flecha-rewrite-build.lock
build=$HOME/.claude/skills/hosts/scripts/agent-build
[[ -r $build ]] || { echo "build wrapper $build is not readable" >&2; exit 2; }
repo=$(git -C "$rewrite" rev-parse --show-toplevel)
head=$(git -C "$repo" rev-parse HEAD)

copy=$(mktemp -d -p /tmp cef-gpui-pump-clippy-guard.XXXXXX)
trap 'rm -rf -- "$copy"' EXIT
git -C "$repo" archive --format=tar "$head" rewrite | tar -x -C "$copy"
mkdir -p "$copy/rewrite/target"
ln -s "$rewrite/target/spike1-src" "$copy/rewrite/target/spike1-src"
(cd "$copy" && patch -p1 --forward --quiet <"$patch_file")
echo "patched a copy of $head in $copy"

status=0
out=$(cd "$copy/rewrite" && CARGO_NET_GIT_FETCH_WITH_CLI=true flock "$lock" \
    bash "$build" run cargo clippy --locked -j 14 --workspace --all-targets -- -D warnings 2>&1) || status=$?
grep -E -A6 '^error: use of a disallowed method' <<<"$out" || true
(( status != 0 )) || { echo "Clippy passed the direct per-call timer" >&2; exit 1; }
grep -c -E '^error: use of a disallowed method `gpui::BackgroundExecutor::timer`' <<<"$out" >/dev/null ||
    { echo "$out" >&2; echo "Clippy failed, but not on the disallowed timer" >&2; exit 1; }
grep -c -F 'spikes/cef-gpui-pump/src/bridge.rs' <<<"$out" >/dev/null ||
    { echo "$out" >&2; echo "the disallowed call is not reported in bridge.rs" >&2; exit 1; }
echo "Clippy rejects the direct per-call timer (exit $status)"
