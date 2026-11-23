# shellcheck shell=bash
# Helpers for ACTIONS scripts run by xvfb-isolated.sh (sourced, not executed).
# WAIT_SECS is required: how long each wait lasts before the step fails. It bounds a test run
# on Xvfb and is not a product limit. Every helper fails (non-zero) on a wrong outcome, so a
# script's `set -e` turns it into a failed run.
: "${WAIT_SECS:?set WAIT_SECS, the per-step wait in seconds}"
xdo() { "${XDOTOOL:-xdotool}" "$@"; }
log=$OUT/stderr.log
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)

# window <name regex>: waits up to WAIT_SECS for a visible window and prints its id.
window() {
    local ids
    if ! ids=$(timeout "$WAIT_SECS" "${XDOTOOL:-xdotool}" search --sync --onlyvisible --name "$1") ||
        [[ -z $ids ]]; then
        echo "FAIL no visible window named /$1/ within ${WAIT_SECS}s" >&2
        return 1
    fi
    printf '%s\n' "${ids%%$'\n'*}"
}

# mark: the current number of log lines. Pass it to wait_for so only later lines count.
mark() { grep -ac '' "$log" || true; }

# wait_for <extended regex> [mark]: waits for a matching log line after line <mark> (default:
# anywhere) and prints the last match.
wait_for() {
    local deadline=$((SECONDS + WAIT_SECS)) hit from=$((${2:-0} + 1))
    while :; do
        hit=$(tail -n "+$from" "$log" | grep -aE "$1" || true)
        if [[ -n $hit ]]; then
            printf '%s\n' "$hit" | tail -n 1
            return 0
        fi
        if ((SECONDS >= deadline)); then
            echo "FAIL waiting for: $1" >&2
            return 1
        fi
        sleep 0.2
    done
}

# count <extended regex> [mark]: number of matching log lines after line <mark>.
count() { tail -n "+$((${2:-0} + 1))" "$log" | grep -acE "$1" || true; }

alive() { kill -0 "$APP_PID" 2>/dev/null; }

send() { printf '%s\n' "$1" >"$CMD"; }

# check <command...>: prints PASS or FAIL and returns the command's status.
check() {
    if "$@"; then
        echo "PASS $*"
    else
        echo "FAIL $*"
        return 1
    fi
}

# wait_exit: waits for the app to exit; the harness then reports its exit status.
wait_exit() {
    local deadline=$((SECONDS + WAIT_SECS))
    while alive; do
        if ((SECONDS >= deadline)); then
            echo "FAIL app still running after ${WAIT_SECS}s" >&2
            return 1
        fi
        sleep 0.2
    done
}

# start_fixture_server: serves scripts/fixture-server.mjs on loopback and sets FIXTURE to its
# base URL. The server stops when the action script exits.
start_fixture_server() {
    local port_file=$OUT/fixture-port deadline=$((SECONDS + WAIT_SECS))
    rm -f "$port_file"
    node "$here/fixture-server.mjs" "$port_file" >"$OUT/fixture-server.log" 2>&1 &
    fixture_pid=$!
    trap 'kill "$fixture_pid" 2>/dev/null || true' EXIT
    until [[ -s $port_file ]]; do
        if ((SECONDS >= deadline)) || ! kill -0 "$fixture_pid" 2>/dev/null; then
            echo "FAIL fixture server did not start; see $OUT/fixture-server.log" >&2
            return 1
        fi
        sleep 0.2
    done
    FIXTURE=http://127.0.0.1:$(<"$port_file")
    export FIXTURE
}
