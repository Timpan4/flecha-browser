#!/usr/bin/env bash
# Shows that run-checks.sh's per-run watchdog ends a hung run and every process
# of it. Runs the idle scenario through the runner, with all its isolation, and
# finds the run's browser through the runner's own process tree. Once the
# browser has a renderer, it records the session's tree with each process's
# group and starts two helpers that ignore SIGTERM, as a Chromium child could:
#   - one joins the session's process group;
#   - one starts its own session (setsid), outside that group, but carries the
#     run's XDG_RUNTIME_DIR, as every process of the run does.
# Then it stops the browser with SIGSTOP so the run hangs. GNU timeout waits
# only for dbus-run-session, so only the runner's own sweep, by group ID and by
# that marker, can end the helpers. Exits nonzero unless every recorded
# session process was in the group, the runner reports the watchdog and fails
# the run, and none of the recorded processes or helpers is left alive.
#
# Scope: what this control can see is the recorded tree, the group and the
# marker. A process that also drops the run's XDG_RUNTIME_DIR from its
# environment is outside all three.
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
alive() { [[ -n $1 ]] && kill -0 "$1" 2>/dev/null; }

# Whatever this script still owns when it exits, on any path: the helpers by
# PID, and the runner, whose EXIT trap ends the run's processes. `jobs -p`
# lists only children not yet reaped, so no PID here can belong to anyone else.
on_exit() {
    local status=$? job
    for job in $(jobs -p); do
        if [[ $job == "${runner:-}" ]]; then kill -TERM "$job" 2>/dev/null || true; else kill -KILL "$job" 2>/dev/null || true; fi
        wait "$job" 2>/dev/null || true
    done
    exit "$status"
}
trap on_exit EXIT
fail() { echo "$1" >&2; exit 1; }

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
[[ -n $browser && -n $renderer ]] || fail "no browser with a renderer under runner $runner within 2 x STEP_TIMEOUT_MS"
pgid=$(ps -o pgid= -p "$browser" | tr -d ' ')
# The group leader is the runner's timeout, so the group is this run's.
tree_of "$runner" | awk -v g="$pgid" '$1 == g' | grep -c . >/dev/null ||
    fail "group leader $pgid is not under runner $runner"
marker=$(tr '\0' '\n' <"/proc/$browser/environ" | grep -E '^XDG_RUNTIME_DIR=/tmp/cef-gpui-pump-run\.[A-Za-z0-9]+/runtime$') ||
    fail "browser $browser has no run XDG_RUNTIME_DIR"
tree_of "$pgid" >"$out/tree-before-stop.txt"

# setpgid(2) may move a process into any group of its own session; the runner
# does not start a new session, so this helper joins the run's group.
perl -e '$SIG{TERM} = "IGNORE"; setpgrp(0, $ARGV[0]) or die "setpgid: $!\n"; sleep' "$pgid" \
    </dev/null >/dev/null 2>"$out/in-group.stderr" &
in_group=$!
# This one leaves for a session of its own, carrying only the run's marker.
env "$marker" perl -MPOSIX -e '$SIG{TERM} = "IGNORE"; POSIX::setsid() > 0 or die "setsid: $!\n"; sleep' \
    </dev/null >/dev/null 2>"$out/own-session.stderr" &
own_session=$!
ready=
for _ in $(seq 1 $(( (step_ms + 99) / 100 ))); do
    if [[ $(ps -o pgid= -p "$in_group" | tr -d ' ') == "$pgid" && $(ps -o sid= -p "$own_session" | tr -d ' ') == "$own_session" ]]; then
        ready=yes
        break
    fi
    sleep 0.1
done
[[ -n $ready ]] || fail "helpers $in_group and $own_session did not take their places"

kill -STOP "$browser" || fail "browser $browser was gone before it could be stopped"
{
    echo "stopped browser $browser (process group $pgid) at $(date +%T.%N | cut -c1-12)"
    echo "session tree under group leader $pgid: $(wc -l <"$out/tree-before-stop.txt") processes (tree-before-stop.txt)"
    echo "helper $in_group joined group $pgid; it ignores SIGTERM"
    echo "helper $own_session runs in its own session with $marker; it ignores SIGTERM"
} | tee "$out/control.log"
outside=$(awk -v g="$pgid" '$3 != g' "$out/tree-before-stop.txt")
echo "session processes outside group $pgid: ${outside:-none}" | tee -a "$out/control.log"

status=0
wait "$runner" || status=$?
runner=
echo "runner exit $status at $(date +%T.%N | cut -c1-12)" | tee -a "$out/control.log"
left_helpers=
for h in "$in_group" "$own_session"; do
    ! alive "$h" || left_helpers+="$h "
done
echo "helpers alive after the runner: ${left_helpers:-none}" | tee -a "$out/control.log"
survivors=$(ps -eo pid=,pgid=,stat=,comm= | awk -v g="$pgid" '$2 == g && $3 !~ /^Z/' || true)
echo "processes left in group $pgid: ${survivors:-none}" | tee -a "$out/control.log"
alive_recorded=
while read -r pid _; do
    ! alive "$pid" || alive_recorded+="$pid "
done <"$out/tree-before-stop.txt"
echo "recorded session processes still alive: ${alive_recorded:-none}" | tee -a "$out/control.log"
[[ -z $outside ]] || fail "session processes left the watchdog's process group"
[[ -z $left_helpers ]] || fail "the runner left helpers alive: $left_helpers"
[[ -z $alive_recorded && -z $survivors ]] || fail "session processes survived"
(( status != 0 )) || fail "the runner passed a hung run"
grep -c 'watchdog expired' "$out/run.log" >/dev/null || fail "no watchdog line in run.log"
[[ -f $out/idle.watchdog.txt ]] || fail "no idle.watchdog.txt"
# The runner's sweep, not timeout's SIGKILL, must be what found both helpers.
for h in "$in_group" "$own_session"; do
    awk -v p="$h" '$1 == p' "$out/idle.leftover.txt" 2>/dev/null | grep -c . >/dev/null ||
        fail "idle.leftover.txt does not list helper $h: the runner's sweep did not find it"
done
echo "watchdog ended the hung run, its process group and its marked processes" | tee -a "$out/control.log"
