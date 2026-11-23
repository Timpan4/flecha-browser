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

# focus <wid> <field id> <x> <y>: clicks the field and waits for the page to report its
# focus event, so typed keys cannot go to the document.
focus() {
    local m
    m=$(mark)
    xdo mousemove --window "$1" "$3" "$4" click 1
    wait_for "^\\[console\\] INPUT focus $2 " "$m" >/dev/null
}

# debug_log_marker_count: the number of FLECHA_CONSOLE_MARKER lines in CEF's own log,
# chrome_debug.log in the profile (PROFILE, default <out-dir>/profile). Call it after the app
# exited. The log may be empty (CEF logs only warnings by default) but must exist and be
# readable, so a moved or unreadable log cannot pass as a clean one.
debug_log_marker_count() {
    local log_file=${PROFILE:-$OUT/profile}/chrome_debug.log status=0
    if [[ ! -f $log_file || ! -r $log_file ]]; then
        echo "FAIL no readable CEF log at $log_file" >&2
        return 1
    fi
    grep -ac 'FLECHA_CONSOLE_MARKER' "$log_file" || status=$?
    # grep: 1 is "no match" (it printed 0); anything else is an error.
    if ((status > 1)); then
        echo "FAIL cannot read $log_file" >&2
        return 1
    fi
}

# fixture_mark: the number of requests the fixture server has logged so far.
fixture_mark() { grep -ac '' "$OUT/fixture-server.log" || true; }

# wait_fixture <extended regex> [fixture mark]: waits for a request line in the fixture
# server's log after line <mark>. The marker page requests /ran after it has logged its
# console marker, so `wait_fixture '^GET /ran'` proves the marker page loaded and its script ran.
wait_fixture() {
    local deadline=$((SECONDS + WAIT_SECS)) from=$((${2:-0} + 1))
    until tail -n "+$from" "$OUT/fixture-server.log" | grep -aEc "$1" >/dev/null; do
        if ((SECONDS >= deadline)); then
            echo "FAIL the fixture server got no request matching: $1" >&2
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
