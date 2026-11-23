#!/usr/bin/env bash
# Shows that run-checks.sh's per-run watchdog ends a hung run. Runs the idle
# scenario through the runner, with all its isolation, and stops the app's
# browser process with SIGSTOP as soon as it exists, so it hangs as a blocked
# CefInitialize would. Exits nonzero unless the runner reports the watchdog,
# fails the run, and leaves no process of the session alive.
#
# Usage: watchdog-control.sh OUT_DIR
#   Takes WARMUP_MS, MEASURE_MS and STEP_TIMEOUT_MS from the environment, as
#   run-checks.sh does.
set -euo pipefail

(( $# == 1 )) || { echo "usage: $0 OUT_DIR" >&2; exit 2; }
: "${WARMUP_MS:?}" "${MEASURE_MS:?}" "${STEP_TIMEOUT_MS:?}"
here=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$1"
out=$(cd "$1" && pwd)

"$here/scripts/run-checks.sh" "$out" idle >"$out/run.log" 2>&1 &
runner=$!
# The browser process is the app binary itself, carrying this run's --pump-out
# and no --type= switch. timeout and dbus-run-session carry the same arguments,
# so matching on the command line alone finds them first.
find_browser() {
    local pid args
    for pid in $(pgrep -x cef-gpui-pump || true); do
        args=$(tr '\0' ' ' <"/proc/$pid/cmdline" 2>/dev/null) || continue
        [[ $args == *"--pump-out=$out/idle.json"* && $args != *--type=* ]] || continue
        echo "$pid"
        return
    done
}
browser=
for _ in $(seq 1 $(( (STEP_TIMEOUT_MS + 99) / 100 ))); do
    browser=$(find_browser)
    [[ -z $browser ]] || break
    sleep 0.1
done
[[ -n $browser ]] || { echo "no browser process within STEP_TIMEOUT_MS" >&2; kill "$runner"; exit 1; }
pgid=$(ps -o pgid= -p "$browser" | tr -d ' ')
kill -STOP "$browser"
echo "stopped browser $browser (process group $pgid) at $(date +%T.%N | cut -c1-12)" | tee "$out/control.log"
status=0
wait "$runner" || status=$?
echo "runner exit $status at $(date +%T.%N | cut -c1-12)" | tee -a "$out/control.log"
survivors=$(ps -eo pid=,pgid=,comm= | awk -v g="$pgid" '$2 == g' || true)
echo "processes left in group $pgid: ${survivors:-none}" | tee -a "$out/control.log"
# Anything that left the group: no app process may run when no other run does,
# and the session's dbus-daemon names the run's scratch dir.
strays=$( (pgrep -a -x cef-gpui-pump; pgrep -a -f 'cef-gpui-pump-run[.]') || true)
echo "app or session processes left anywhere: ${strays:-none}" | tee -a "$out/control.log"
[[ -z $strays ]] || { echo "processes outside the group survived" >&2; exit 1; }
(( status != 0 )) || { echo "the runner passed a hung run" >&2; exit 1; }
grep -c 'watchdog expired' "$out/run.log" >/dev/null || { echo "no watchdog line in run.log" >&2; exit 1; }
[[ -f $out/idle.watchdog.txt ]] || { echo "no idle.watchdog.txt" >&2; exit 1; }
[[ -z $survivors ]] || { echo "processes survived the watchdog" >&2; exit 1; }
! kill -0 "$browser" 2>/dev/null || { echo "browser $browser survived" >&2; exit 1; }
echo "watchdog ended the hung run and its process group" | tee -a "$out/control.log"
