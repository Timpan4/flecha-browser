# shellcheck shell=bash
# Sourced by run-checks.sh and test-sweep.sh: how the runner finds and ends
# what is left of one app run. A run is the members of its session's process
# group and every process marked with its scratch XDG_RUNTIME_DIR, which a
# process keeps when it leaves the group or is reparented. The app binary's
# --pump-sweep modes (src/sweep.rs) find marked processes and kill them through
# pidfds after re-checking each one's start time and marker, since Bash cannot
# open a pidfd. A process that hides its environment (non-dumpable) or drops
# the marker is outside both, so the sweep is not complete.
#
# Every caller runs these in an `||` list, where errexit is off, so each step
# checks its own status.

# The live processes of one run, one `pid pgid state args` line each: the
# members of group $2 (none if empty) and the processes the sweeper $1 finds
# marked with runtime dir $3 (none if empty). Zombies are left out. Returns
# nonzero, printing nothing, if either inventory failed.
run_processes() { # sweeper group runtime
    local members='' marked=''
    if [[ -n $2 ]]; then
        members=$(ps -eo pid=,pgid=,stat=,args= | awk -v g="$2" '$2 == g && $3 !~ /^Z/') || return 1
    fi
    if [[ -n $3 ]]; then
        [[ -x $1 ]] || return 1
        marked=$(env -i "$1" --pump-sweep="$3") || return 1
    fi
    printf '%s\n%s\n' "$members" "$marked" | awk 'NF && !seen[$1]++'
}

now_ms() { local t=${EPOCHREALTIME//[!0-9]/}; echo $((10#$t / 1000)); }

# Ends what is left of a run: group $2 by its ID, while the inventory lists a
# member of it (an empty group's ID can be reused once its leader is reaped),
# and the processes marked with runtime dir $3 through the sweeper $1. Group
# members get no per-PID signal. Waits until none is left, for at most
# STEP_TIMEOUT_MS. Prints what it found and the sweeper's outcomes. Returns 0
# if nothing was left, 1 if something was and it is gone, 2 if any survived,
# 3 if an inventory or the marker sweep failed, so the sweep is incomplete.
end_group() { # sweeper group runtime
    local sweeper=$1 group=$2 runtime=$3 left deadline
    if ! left=$(run_processes "$sweeper" "$group" "$runtime"); then
        echo "sweep incomplete: the inventory failed"
        return 3
    fi
    [[ -n $left ]] || return 0
    printf '%s\n' "$left"
    deadline=$(( $(now_ms) + STEP_TIMEOUT_MS ))
    while :; do
        if [[ -n $group ]] && awk -v g="$group" '$2 == g { f = 1 } END { exit !f }' <<<"$left"; then
            kill -KILL -- "-$group" 2>/dev/null || true
        fi
        if [[ -n $runtime ]] && ! env -i "$sweeper" --pump-sweep-kill="$runtime"; then
            echo "sweep incomplete: the marker sweep failed"
            return 3
        fi
        if ! left=$(run_processes "$sweeper" "$group" "$runtime"); then
            echo "sweep incomplete: the inventory failed"
            return 3
        fi
        [[ -n $left ]] || return 1
        (( $(now_ms) < deadline )) || break
        sleep 0.1
    done
    printf '%s\n' "$left" | sed 's/^/still alive: /'
    return 2
}

# Stops the run in flight when the runner exits: every unreaped child except
# Xvfb $2 (the session's launcher, which may not have made its group yet), then
# group $3 and the processes marked with runtime dir $4. Sweeps by marker
# whenever $4 is set, even with no group: the launcher may have exited and been
# reaped before its group was recorded.
stop_run() { # sweeper xvfb_pid group runtime
    local job
    for job in $(jobs -p); do
        [[ $job != "$2" ]] || continue
        kill -KILL "$job" 2>/dev/null || true
        # timeout calls setpgid(0, 0) before it forks (coreutils src/timeout.c).
        # Until the reap below, its PID, and so its group ID, stays reserved.
        kill -KILL -- "-$job" 2>/dev/null || true
        wait "$job" 2>/dev/null || true
        end_group "$1" "$job" "$4" || true
    done
    if [[ -n $3 || -n $4 ]]; then end_group "$1" "$3" "$4" || true; fi
}
