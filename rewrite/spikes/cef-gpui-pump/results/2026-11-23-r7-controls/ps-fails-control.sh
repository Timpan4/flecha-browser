#!/usr/bin/env bash
# Shows that run-checks.sh fails a run whose end-of-run inventory fails. Puts a
# `ps` that always fails, with no output, first on the runner's PATH and runs
# the idle scenario through the runner with its full isolation. The app never
# runs `ps`, so the run itself passes; only the runner's sweep sees the
# failure. Exits nonzero unless the runner exits nonzero, reports the
# incomplete sweep, and the run's leftover file says the inventory failed.
#
# Usage: ps-fails-control.sh OUT_DIR
#   Takes WARMUP_MS, MEASURE_MS and STEP_TIMEOUT_MS from the environment, as
#   run-checks.sh does.
set -euo pipefail

(( $# == 1 )) || { echo "usage: $0 OUT_DIR" >&2; exit 2; }
: "${WARMUP_MS:?}" "${MEASURE_MS:?}" "${STEP_TIMEOUT_MS:?}"
here=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$1"
out=$(cd "$1" && pwd)
shim=$(mktemp -d -p /tmp cef-gpui-pump-ps-shim.XXXXXX)
[[ $shim =~ ^/tmp/cef-gpui-pump-ps-shim\.[A-Za-z0-9]+$ ]] || { echo "unexpected shim dir $shim" >&2; exit 1; }
trap 'rm -rf -- "$shim"' EXIT
printf '#!/bin/sh\necho "ps shim: failing" >&2\nexit 1\n' >"$shim/ps"
chmod +x "$shim/ps"
fail() { echo "FAIL: $1" >&2; exit 1; }

status=0
PATH=$shim:$PATH "$here/scripts/run-checks.sh" "$out" idle >"$out/run.log" 2>&1 || status=$?
echo "runner exit $status" | tee "$out/control.log"
grep -F 'exit=' "$out/run.log" | tee -a "$out/control.log"
(( status != 0 )) || fail "the runner passed a run whose inventory failed"
grep -Fc 'sweep of process group' "$out/run.log" >/dev/null || fail "run.log does not report the incomplete sweep"
grep -Fxc 'sweep incomplete: the inventory failed' "$out/idle.leftover.txt" >/dev/null ||
    fail "idle.leftover.txt does not say the inventory failed"
echo "the runner failed the run because its inventory failed" | tee -a "$out/control.log"
