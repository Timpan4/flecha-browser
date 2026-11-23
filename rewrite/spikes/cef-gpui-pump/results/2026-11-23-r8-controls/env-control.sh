#!/usr/bin/env bash
# Shows that none of the caller's session variables reach a run's processes.
# Sets SSH_AUTH_SOCK, AT_SPI_BUS_ADDRESS, PULSE_SERVER and XAUTHORITY in the
# runner's environment, each pointing at a path under OUT_DIR that does not
# exist, and runs the idle scenario through the runner with its full isolation.
# While the run is up it reads /proc/PID/environ of the browser (the
# `cef-gpui-pump` process with no --type= below the runner) and of every other
# process below the runner's `timeout` whose environment it can read. Exits
# nonzero unless the browser's environment was read, carries the run's scratch
# XDG_RUNTIME_DIR, and no read environment holds any of the four names.
# Writes the variable names it saw per process, never their values.
#
# Usage: env-control.sh OUT_DIR [RUNNER]
#   RUNNER defaults to this checkout's scripts/run-checks.sh. Takes WARMUP_MS,
#   MEASURE_MS and STEP_TIMEOUT_MS from the environment, as run-checks.sh does.
set -euo pipefail

(( $# == 1 || $# == 2 )) || { echo "usage: $0 OUT_DIR [RUNNER]" >&2; exit 2; }
: "${WARMUP_MS:?}" "${MEASURE_MS:?}" "${STEP_TIMEOUT_MS:?}"
[[ $STEP_TIMEOUT_MS =~ ^[0-9]+$ ]] || { echo "STEP_TIMEOUT_MS must be digits" >&2; exit 2; }
step_ms=$((10#$STEP_TIMEOUT_MS))
here=$(cd "$(dirname "$0")/../.." && pwd)
runner_script=${2:-$here/scripts/run-checks.sh}
[[ -x $runner_script ]] || { echo "runner $runner_script is not executable" >&2; exit 2; }
mkdir -p "$1"
out=$(cd "$1" && pwd)
canaries=(SSH_AUTH_SOCK AT_SPI_BUS_ADDRESS PULSE_SERVER XAUTHORITY)

# PID, parent and command name of a process and all its descendants.
tree_of() {
    ps -eo pid=,ppid=,comm=,args= | awk -v root="$1" '
        { pid[NR] = $1; ppid[NR] = $2; line[NR] = $0 }
        END {
            keep[root] = 1
            do {
                grown = 0
                for (i = 1; i <= NR; i++)
                    if (!(pid[i] in keep) && (ppid[i] in keep)) { keep[pid[i]] = 1; grown = 1 }
            } while (grown)
            for (i = 1; i <= NR; i++) if (pid[i] in keep) print line[i]
        }'
}
names_of() { tr '\0' '\n' <"/proc/$1/environ" | sed -n 's/=.*//p' | sort; }

on_exit() {
    local status=$? job
    for job in $(jobs -p); do
        kill -TERM "$job" 2>/dev/null || true
        wait "$job" 2>/dev/null || true
    done
    exit "$status"
}
trap on_exit EXIT
fail() { echo "FAIL: $1" | tee -a "$out/control.log" >&2; exit 1; }
: >"$out/control.log"

env SSH_AUTH_SOCK="$out/canary/ssh-agent.sock" AT_SPI_BUS_ADDRESS="unix:path=$out/canary/at-spi-bus" \
    PULSE_SERVER="unix:$out/canary/pulse-native" XAUTHORITY="$out/canary/Xauthority" \
    "$runner_script" "$out" idle >"$out/run.log" 2>&1 &
runner=$!
# The browser comes from CefInitialize, which the runner's watchdog budgets one
# STEP_TIMEOUT_MS after its setup steps, each bounded by the runner itself.
browser=
while kill -0 "$runner" 2>/dev/null; do
    browser=$(tree_of "$runner" | awk '$3 == "cef-gpui-pump" && !/--type=/ { print $1; exit }')
    [[ -z $browser ]] || break
    sleep 0.1
done
[[ -n $browser ]] || fail "no browser under runner $runner; see run.log"
# Let the browser start its children, for at most one STEP_TIMEOUT_MS.
for _ in $(seq 1 $(( (step_ms + 99) / 100 ))); do
    tree_of "$browser" | awk '/--type=zygote/' | grep -c . >/dev/null && break
    sleep 0.1
done
names_of "$browser" >"$out/browser-environ-names.txt" || fail "cannot read the browser's environ"
marker=$(tr '\0' '\n' <"/proc/$browser/environ" | grep -cE '^XDG_RUNTIME_DIR=/tmp/cef-gpui-pump-run\.[A-Za-z0-9]+/runtime$' || true)
session=$(tree_of "$runner" | awk '$3 == "timeout" { print $1; exit }')
[[ -n $session ]] || fail "no timeout below runner $runner"
read_count=0 unreadable=()
: >"$out/session-environ-names.txt"
while read -r pid _ comm _; do
    if names=$(names_of "$pid" 2>/dev/null); then
        read_count=$((read_count + 1))
        printf '%s %s: %s\n' "$pid" "$comm" "$(tr '\n' ' ' <<<"$names")" >>"$out/session-environ-names.txt"
    else
        unreadable+=("$pid:$comm")
    fi
done < <(tree_of "$session" | awk -v s="$session" '$1 != s')
status=0
wait "$runner" || status=$?
runner=
{
    echo "browser $browser, runner exit $status"
    echo "browser environ: $(wc -l <"$out/browser-environ-names.txt") variables: $(tr '\n' ' ' <"$out/browser-environ-names.txt")"
    echo "session processes below timeout $session: $read_count read, unreadable: ${unreadable[*]:-none}"
} | tee -a "$out/control.log"
(( marker == 1 )) || fail "the browser's environ has no run XDG_RUNTIME_DIR"
leaked=
for name in "${canaries[@]}"; do
    if grep -Fxc "$name" "$out/browser-environ-names.txt" >/dev/null || grep -Ec "[: ]$name( |\$)" "$out/session-environ-names.txt" >/dev/null; then
        leaked+="$name "
    fi
done
[[ -z $leaked ]] || fail "caller variables reached the run: $leaked"
echo "none of ${canaries[*]} reached the browser or any readable session process" | tee -a "$out/control.log"
