# Helpers for ACTIONS scripts run by xvfb-isolated.sh (sourced, not executed).
# WAIT_SECS is required: how long wait_for waits for a log line before failing. It bounds a
# test run on Xvfb and is not a product limit.
x=${XDOTOOL:-xdotool}
: "${WAIT_SECS:?set WAIT_SECS, the per-step wait in seconds}"
log=$OUT/stderr.log

# wait_for <extended regex>: waits until stderr.log has a matching line and prints the last one.
wait_for() {
    local deadline=$((SECONDS + WAIT_SECS)) hit
    while :; do
        hit=$(grep -aE "$1" "$log" || true)
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

# count <extended regex>: number of matching stderr.log lines.
count() { grep -acE "$1" "$log" || true; }

alive() { kill -0 "$APP_PID" 2>/dev/null; }

send() { printf '%s\n' "$1" >"$CMD"; }

check() {
    if "$@"; then echo "PASS $*"; else echo "FAIL $*"; fi
}

# wait_exit: waits for the app to exit; the harness then reports its exit status.
wait_exit() {
    local deadline=$((SECONDS + WAIT_SECS))
    while alive; do
        if ((SECONDS >= deadline)); then
            echo "FAIL app still running" >&2
            return 1
        fi
        sleep 0.2
    done
}
