#!/usr/bin/env bash
# Runs the spike on its own Xvfb display, never on the desktop session.
#
#   WAIT_SECS=<secs> xvfb-isolated.sh <out-dir> <cef-osr-signin args...>
#
# Isolation, for this script and everything it starts (Xvfb, the app, ACTIONS and any window
# manager): no session bus address, a system bus address that points nowhere, no keyring or
# desktop-session variables, scratch HOME, XDG, runtime and temp (TMPDIR) dirs under <out-dir>, no
# inherited LD_LIBRARY_PATH, WAYLAND_DISPLAY unset. The app gets a private D-Bus session bus
# without service activation (session-bus.conf) and is started through run.sh, which loads the
# runtime prepare.sh staged. Logs go to <out-dir>/stdout.log and stderr.log. The app's stdin
# is the FIFO <out-dir>/stdin.fifo, so OSR terminal commands can be sent with
# `echo 'go <url>' >"$CMD"`.
#
# WAIT_SECS is required. It bounds each wait: Xvfb readiness, the first page load, and the
# grace period between SIGTERM and SIGKILL when the app is stopped. It bounds a test run and
# is not a product limit.
#
# Without SCREENSHOT or ACTIONS the script waits for the app to exit and returns its status.
# With SCREENSHOT=1 or ACTIONS=<script> it waits for the app's first "[load-end]" line, runs
# ACTIONS (with DISPLAY, OUT, APP_PID and CMD set), then saves <out-dir>/screenshot.png. If the
# actions ended the app, the script returns the app's exit status; otherwise it stops the app.
# A failing ACTIONS script fails the run.
set -euo pipefail
: "${WAIT_SECS:?set WAIT_SECS, the bound in seconds for each wait}"

here=$(cd "$(dirname "$0")" && pwd)
# LAUNCHER overrides the app launcher (auto-checks.sh uses flecha-launch.sh for the Flecha
# reference column); it gets the same isolated environment and arguments.
launcher=${LAUNCHER:-$here/../run.sh}
out=$(mkdir -p "$1" && cd "$1" && pwd)
shift
mkdir -p "$out"/{home,xdg-config,xdg-cache,xdg-data,runtime,tmp}
chmod 700 "$out/runtime" "$out/tmp"
# Chromium creates its process-singleton socket at
# $TMPDIR/org.chromium.Chromium.XXXXXX/SingletonSocket, and a Unix socket path holds at most
# 107 bytes (sun_path is 108 bytes with the NUL, unix(7)).
singleton=$out/tmp/org.chromium.Chromium.XXXXXX/SingletonSocket
if ((${#singleton} > 107)); then
    echo "<out-dir> is too long for Chromium's singleton socket (${#singleton} of 107 bytes): $out" >&2
    exit 2
fi

unset DBUS_SESSION_BUS_ADDRESS WAYLAND_DISPLAY LD_LIBRARY_PATH SSH_AUTH_SOCK \
    GNOME_KEYRING_CONTROL XDG_CURRENT_DESKTOP XDG_SESSION_DESKTOP DESKTOP_SESSION KDE_FULL_SESSION
# libdbus and GDBus fall back to /run/dbus/system_bus_socket without this; a path that does
# not exist keeps every process off the host's system bus.
export DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent/flecha-spike-no-system-bus
export HOME=$out/home XDG_RUNTIME_DIR=$out/runtime XDG_CONFIG_HOME=$out/xdg-config \
    XDG_CACHE_HOME=$out/xdg-cache XDG_DATA_HOME=$out/xdg-data XDG_SESSION_TYPE=x11 TMPDIR=$out/tmp

display_fifo=$out/display.fifo
cmd_fifo=$out/stdin.fifo
rm -f "$display_fifo" "$cmd_fifo"
mkfifo "$display_fifo" "$cmd_fifo"
# Read-write opens do not block, so a stalled Xvfb cannot hang the harness on open, and the
# command FIFO stays open between commands (the app sees no EOF).
exec 3<>"$display_fifo" 4<>"$cmd_fifo"

xvfb_pid=
app_pid=
# Sends TERM to a whole process group, waits up to WAIT_SECS, then sends KILL.
# shellcheck disable=SC2329 # called from cleanup, which runs through trap EXIT
stop_group() {
    local pgid=$1 deadline=$((SECONDS + WAIT_SECS))
    kill -TERM -- "-$pgid" 2>/dev/null || return 0
    while kill -0 -- "-$pgid" 2>/dev/null; do
        if ((SECONDS >= deadline)); then
            echo "process group $pgid ignored SIGTERM for ${WAIT_SECS}s; sending SIGKILL" >&2
            kill -KILL -- "-$pgid" 2>/dev/null || true
            break
        fi
        sleep 0.2
    done
}
# shellcheck disable=SC2329 # runs through trap EXIT
cleanup() {
    if [[ -n $app_pid ]]; then
        stop_group "$app_pid"
        wait "$app_pid" 2>/dev/null || true
    fi
    if [[ -n $xvfb_pid ]]; then
        stop_group "$xvfb_pid"
        wait "$xvfb_pid" 2>/dev/null || true
    fi
    exec 3>&- 4>&-
    rm -f "$display_fifo" "$cmd_fifo"
}
trap cleanup EXIT

# setsid gives Xvfb and the app their own process groups (pgid = pid), so cleanup reaches
# every process they start.
setsid Xvfb -displayfd 3 -nolisten tcp 2>"$out/xvfb.log" &
xvfb_pid=$!
if ! read -r -t "$WAIT_SECS" display_number <&3; then
    echo "Xvfb published no display within ${WAIT_SECS}s; see $out/xvfb.log" >&2
    exit 1
fi
display=:$display_number
export DISPLAY=$display
echo "xvfb pid $xvfb_pid display $display"

setsid dbus-run-session --config-file="$here/session-bus.conf" -- "$launcher" "$@" \
    <"$cmd_fifo" >"$out/stdout.log" 2>"$out/stderr.log" &
app_pid=$!
echo "app pid $app_pid"

finish() {
    set +e
    wait "$app_pid"
    local status=$?
    set -e
    app_pid=
    echo "app exit $status"
    exit "$status"
}

if [[ ${SCREENSHOT:-0} == 1 || -n ${ACTIONS:-} ]]; then
    deadline=$((SECONDS + WAIT_SECS))
    until grep -ac '^\[load-end\] ' "$out/stderr.log" >/dev/null; do
        if ! kill -0 "$app_pid" 2>/dev/null; then
            set +e
            wait "$app_pid"
            status=$?
            app_pid=
            echo "app exited with status $status before loading a page" >&2
            exit $((status == 0 ? 1 : status))
        fi
        if ((SECONDS >= deadline)); then
            echo "no page load within ${WAIT_SECS}s" >&2
            exit 1
        fi
        sleep 0.2
    done
    if [[ -n ${ACTIONS:-} ]]; then
        OUT=$out APP_PID=$app_pid CMD=$cmd_fifo bash "$ACTIONS"
    fi
    if ! kill -0 "$app_pid" 2>/dev/null; then
        finish
    fi
    import -window root "$out/screenshot.png"
    echo "screenshot $out/screenshot.png"
    exit 0
fi

finish
