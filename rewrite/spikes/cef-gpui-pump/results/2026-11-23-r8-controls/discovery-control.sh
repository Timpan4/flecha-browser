#!/usr/bin/env bash
# Shows that process discovery fails closed. Applies `root-stat-fails.patch`
# (mutant D: the browser's own /proc/PID/stat read fails with EMFILE) to a
# disposable copy of the committed tree (`git archive HEAD`), never the
# caller's checkout, builds it, and runs the copy's runner with its full
# isolation:
#   - idle with EXTRA_ARGS=--pump-forbid-paths=/tmp, the verifier's dirty run,
#     which mutant D passed at 56d12e9;
#   - cdp, whose listener scan mutant D reported complete at 56d12e9.
# Exits nonzero unless the runner fails both runs, the idle report has
# isolation_clean and shutdown_clean false with discovery errors naming the
# root, and the cdp report has listening_sockets.complete and cdp_pass false.
# The copy is deleted on exit.
#
# Usage: discovery-control.sh REWRITE_DIR OUT_DIR
#   REWRITE_DIR  the rewrite/ dir of a checkout whose target/spike1-src was
#                staged by spikes/gpui-cef-compositor/prepare.sh
#   Takes WARMUP_MS, MEASURE_MS and STEP_TIMEOUT_MS from the environment.
set -euo pipefail

(( $# == 2 )) || { echo "usage: $0 REWRITE_DIR OUT_DIR" >&2; exit 2; }
: "${WARMUP_MS:?}" "${MEASURE_MS:?}" "${STEP_TIMEOUT_MS:?}"
rewrite=$(cd "$1" && pwd)
here=$(cd "$(dirname "$0")" && pwd)
[[ -d $rewrite/target/spike1-src ]] || { echo "$rewrite/target/spike1-src is missing; run prepare.sh first" >&2; exit 2; }
: "${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set for the build lock}"
lock=$XDG_RUNTIME_DIR/flecha-rewrite-build.lock
build=$HOME/.claude/skills/hosts/scripts/agent-build
[[ -r $build ]] || { echo "build wrapper $build is not readable" >&2; exit 2; }
mkdir -p "$2"
out=$(cd "$2" && pwd)
repo=$(git -C "$rewrite" rev-parse --show-toplevel)
head=$(git -C "$repo" rev-parse HEAD)

copy=$(mktemp -d -p /tmp cef-gpui-pump-discovery.XXXXXX)
[[ $copy =~ ^/tmp/cef-gpui-pump-discovery\.[A-Za-z0-9]+$ ]] || { echo "unexpected copy dir $copy" >&2; exit 1; }
trap 'rm -rf -- "$copy"' EXIT
git -C "$repo" archive --format=tar "$head" rewrite | tar -x -C "$copy"
mkdir -p "$copy/rewrite/target"
ln -s "$rewrite/target/spike1-src" "$copy/rewrite/target/spike1-src"
(cd "$copy" && patch -p1 --forward --quiet <"$here/root-stat-fails.patch")
echo "mutant D on a copy of $head in $copy" | tee "$out/control.log"
(cd "$copy/rewrite" && CARGO_NET_GIT_FETCH_WITH_CLI=true flock "$lock" \
    bash "$build" run cargo build --locked --release -j 14 -p cef-gpui-pump) >"$out/build.log" 2>&1 ||
    { tail -20 "$out/build.log" >&2; echo "mutant build failed" >&2; exit 1; }
runner=$copy/rewrite/spikes/cef-gpui-pump/scripts/run-checks.sh
fail() { echo "FAIL: $1" | tee -a "$out/control.log" >&2; exit 1; }

status=0
EXTRA_ARGS=--pump-forbid-paths=/tmp "$runner" "$out/dirty-idle" idle >"$out/dirty-idle.log" 2>&1 || status=$?
r=$out/dirty-idle/idle.json
echo "dirty idle: runner exit $status; $(grep -F 'exit=' "$out/dirty-idle.log" | sed 's/^ *//')" | tee -a "$out/control.log"
[[ -f $r ]] || fail "no idle report"
jq -c '{isolation_clean, shutdown_clean, pass, after_load: .isolation_after_load.discovery_errors, shutdown: .shutdown.discovery_errors}' "$r" | tee -a "$out/control.log"
(( status != 0 )) || fail "the runner passed the dirty idle run"
[[ $(jq '.isolation_clean == false and .shutdown_clean == false and .pass == false' "$r") == true ]] ||
    fail "isolation_clean, shutdown_clean or pass is not false"
[[ $(jq '[.isolation_after_load.discovery_errors[], .shutdown.discovery_errors[]] | map(test("root [0-9]+ is not")) | any' "$r") == true ]] ||
    fail "no discovery error names the missing root"

status=0
"$runner" "$out/cdp" cdp >"$out/cdp.log" 2>&1 || status=$?
r=$out/cdp/cdp.json
echo "cdp: runner exit $status; $(grep -F 'exit=' "$out/cdp.log" | sed 's/^ *//')" | tee -a "$out/control.log"
[[ -f $r ]] || fail "no cdp report"
jq -c '{cdp_pass, isolation_clean, shutdown_clean, pass, listening_complete: .listening_sockets.complete, listening_discovery: .listening_sockets.discovery_errors}' "$r" | tee -a "$out/control.log"
(( status != 0 )) || fail "the runner passed the cdp run"
[[ $(jq '.listening_sockets.complete == false and .cdp_pass == false and .pass == false' "$r") == true ]] ||
    fail "listening_sockets.complete, cdp_pass or pass is not false"
echo "mutant D fails the dirty idle run and the cdp run on discovery errors" | tee -a "$out/control.log"
