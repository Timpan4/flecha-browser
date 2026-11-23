#!/usr/bin/env bash
# Shows that run-checks.sh's per-run watchdog ends a hung run and everything in
# its session's process group. Runs the idle scenario through the runner, with
# all its isolation, and finds the run's browser through the runner's own
# process tree. Once the browser has a renderer, it records the session's tree
# with each process's group, adds one process to that group that ignores
# SIGTERM (as a Chromium child could), and stops the browser with SIGSTOP so
# the run hangs. GNU timeout waits only for dbus-run-session, so only the
# runner's own group kill can end the SIGTERM-ignoring process. Exits nonzero
# unless every session process was in the group, the runner reports the
# watchdog and fails the run, and none of those processes is left alive.
#
# Usage: watchdog-control.sh OUT_DIR
#   Takes WARMUP_MS, MEASURE_MS and STEP_TIMEOUT_MS from the environment, as
#   run-checks.sh does.
set -euo pipefail

(( $# == 1 )) || { echo "usage: $0 OUT_DIR" >&2; exit 2; }
: "${WARMUP_MS:?}" "${MEASURE_MS:?}" "${STEP_TIMEOUT_MS:?}"
[[ $STEP_TIMEOUT_MS =~ ^[0-9]+$ ]] || { echo "STEP_TIMEOUT_MS must be digits" >&2; exit 2; }
step_ms=$((10#$STEP_TIMEOUT_MS))
here=$(cd "$(dirname "$0")/../.." && pwd)
mkdir -p "$1"
out=$(cd "$1" && pwd)

# PID, parent, group, command name and arguments of a process and all its
# descendants.
tree_of() {
    ps -eo pid=,ppid=,pgid=,comm=,args= | awk -v root="$1" '
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
# The browser process: the app binary with no --type= switch.
browser_in() { awk '$4 == "cef-gpui-pump" && !/--type=/ { print $1; exit }'; }
# A renderer: the zygote forks it, so it keeps `--type=zygote` and its parent
# and grandparent are zygotes too (`probe.rs` classifies it the same way).
forked_renderer() {
    awk '{ zyg[$1] = ($0 ~ /--type=zygote/); parent[$1] = $2; order[NR] = $1 }
        END {
            for (i = 1; i <= NR; i++) {
                p = order[i]
                if (zyg[p] && zyg[parent[p]] && zyg[parent[parent[p]]]) { print p; exit }
            }
        }'
}
resist=
abort() {
    echo "$1" >&2
    [[ -z $resist ]] || kill -KILL "$resist" 2>/dev/null || true
    # The runner's EXIT trap ends the run's process group.
    kill "$runner" 2>/dev/null || true
    wait "$runner" 2>/dev/null || true
    exit 1
}

"$here/scripts/run-checks.sh" "$out" idle >"$out/run.log" 2>&1 &
runner=$!
# The browser and its first renderer come from CefInitialize and
# CreateBrowserSync, which the runner's watchdog budgets one STEP_TIMEOUT_MS
# each, so wait for them that long.
browser=
renderer=
for _ in $(seq 1 $(( (2 * step_ms + 99) / 100 ))); do
    [[ -n $browser ]] || browser=$(tree_of "$runner" | browser_in)
    if [[ -n $browser ]]; then
        renderer=$(tree_of "$browser" | forked_renderer)
        [[ -z $renderer ]] || break
    fi
    sleep 0.1
done
[[ -n $browser && -n $renderer ]] || abort "no browser with a renderer under runner $runner within 2 x STEP_TIMEOUT_MS"
pgid=$(ps -o pgid= -p "$browser" | tr -d ' ')
# The group leader is the runner's timeout, so the group is this run's.
tree_of "$runner" | awk -v g="$pgid" '$1 == g' | grep -c . >/dev/null ||
    abort "group leader $pgid is not under runner $runner"
tree_of "$pgid" >"$out/tree-before-stop.txt"

# setpgid(2) may move a process into any group of its own session; the runner
# does not start a new session, so this process joins the run's group.
perl -e '$SIG{TERM} = "IGNORE"; setpgrp(0, $ARGV[0]) or die "setpgid: $!\n"; sleep' "$pgid" \
    </dev/null >/dev/null 2>"$out/resist.stderr" &
resist=$!
joined=
for _ in $(seq 1 $(( (step_ms + 99) / 100 ))); do
    [[ $(ps -o pgid= -p "$resist" | tr -d ' ') != "$pgid" ]] || { joined=yes; break; }
    sleep 0.1
done
[[ -n $joined ]] || abort "SIGTERM-ignoring process $resist did not join group $pgid"

kill -STOP "$browser"
{
    echo "stopped browser $browser (process group $pgid) at $(date +%T.%N | cut -c1-12)"
    echo "session tree under group leader $pgid: $(wc -l <"$out/tree-before-stop.txt") processes (tree-before-stop.txt)"
    echo "added process $resist to group $pgid; it ignores SIGTERM"
} | tee "$out/control.log"
outside=$(awk -v g="$pgid" '$3 != g' "$out/tree-before-stop.txt")
echo "session processes outside group $pgid: ${outside:-none}" | tee -a "$out/control.log"

status=0
wait "$runner" || status=$?
echo "runner exit $status at $(date +%T.%N | cut -c1-12)" | tee -a "$out/control.log"
resist_alive=
if kill -0 "$resist" 2>/dev/null; then
    resist_alive=yes
    kill -KILL "$resist"
fi
wait "$resist" 2>/dev/null || true
echo "SIGTERM-ignoring process $resist alive after the runner: ${resist_alive:-no}" | tee -a "$out/control.log"
survivors=$(ps -eo pid=,pgid=,comm= | awk -v g="$pgid" '$2 == g' || true)
echo "processes left in group $pgid: ${survivors:-none}" | tee -a "$out/control.log"
alive=
while read -r pid _; do
    ! kill -0 "$pid" 2>/dev/null || alive+="$pid "
done <"$out/tree-before-stop.txt"
echo "recorded session processes still alive: ${alive:-none}" | tee -a "$out/control.log"
[[ -z $outside ]] || { echo "session processes left the watchdog's process group" >&2; exit 1; }
[[ -z $resist_alive ]] || { echo "the runner left the SIGTERM-ignoring process alive" >&2; exit 1; }
[[ -z $alive && -z $survivors ]] || { echo "session processes survived" >&2; exit 1; }
(( status != 0 )) || { echo "the runner passed a hung run" >&2; exit 1; }
grep -c 'watchdog expired' "$out/run.log" >/dev/null || { echo "no watchdog line in run.log" >&2; exit 1; }
[[ -f $out/idle.watchdog.txt ]] || { echo "no idle.watchdog.txt" >&2; exit 1; }
# The runner's end_group, not timeout's SIGKILL, must be what found it.
awk -v p="$resist" '$1 == p' "$out/idle.leftover.txt" 2>/dev/null | grep -c . >/dev/null ||
    { echo "idle.leftover.txt does not list $resist: the runner's group kill was not exercised" >&2; exit 1; }
echo "watchdog ended the hung run and its whole process group" | tee -a "$out/control.log"
