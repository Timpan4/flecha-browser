#!/usr/bin/env bash
# Runs the spike on its own Xvfb display, never on the desktop session.
#
#   xvfb-isolated.sh <out-dir> <cef-osr-signin args...>
#
# Isolation: a private D-Bus session bus without service activation (session-bus.conf),
# scratch HOME, XDG and runtime dirs under <out-dir>, no inherited LD_LIBRARY_PATH, and
# WAYLAND_DISPLAY unset. Logs go to <out-dir>/stdout.log and stderr.log. The app's stdin is
# the FIFO <out-dir>/stdin.fifo, so OSR terminal commands can be sent with
# `echo 'go <url>' >"$CMD"`. Xvfb and the app are stopped by PID.
#
# Without SCREENSHOT or ACTIONS the script waits for the app to exit and returns its status.
# With SCREENSHOT=1 or ACTIONS=<script> it waits for the app's first "[load-end]" line, runs
# ACTIONS (with DISPLAY, OUT, APP_PID and CMD set), then saves <out-dir>/screenshot.png. If the
# actions ended the app, the script returns the app's exit status; otherwise it stops the app.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
bin=${BIN:-$here/../../../target/debug/cef-osr-signin}
out=$1
shift
mkdir -p "$out"/{home,xdg-config,xdg-cache,xdg-data,runtime}
chmod 700 "$out/runtime"

display_fifo=$out/display.fifo
cmd_fifo=$out/stdin.fifo
rm -f "$display_fifo" "$cmd_fifo"
mkfifo "$display_fifo" "$cmd_fifo"
# Hold the command FIFO open so the app sees no EOF between commands.
exec 4<>"$cmd_fifo"

Xvfb -displayfd 3 -nolisten tcp 3>"$display_fifo" 2>"$out/xvfb.log" &
xvfb_pid=$!
app_pid=
cleanup() {
    if [[ -n $app_pid ]]; then
        # The browser is dbus-run-session's child; stop it first so it is not orphaned.
        for child in $(pgrep -P "$app_pid" || true); do kill "$child" 2>/dev/null || true; done
        wait "$app_pid" 2>/dev/null || true
    fi
    kill "$xvfb_pid" 2>/dev/null || true
    wait "$xvfb_pid" 2>/dev/null || true
    exec 4>&-
    rm -f "$display_fifo" "$cmd_fifo"
}
trap cleanup EXIT
read -r display_number <"$display_fifo"
display=:$display_number
echo "xvfb pid $xvfb_pid display $display"

env -u WAYLAND_DISPLAY -u DBUS_SESSION_BUS_ADDRESS -u LD_LIBRARY_PATH \
    DISPLAY="$display" HOME="$out/home" XDG_RUNTIME_DIR="$out/runtime" \
    XDG_CONFIG_HOME="$out/xdg-config" XDG_CACHE_HOME="$out/xdg-cache" \
    XDG_DATA_HOME="$out/xdg-data" XDG_SESSION_TYPE=x11 \
    dbus-run-session --config-file="$here/session-bus.conf" -- "$bin" "$@" \
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
    until grep -ac '^\[load-end\] ' "$out/stderr.log" >/dev/null; do
        kill -0 "$app_pid" 2>/dev/null || { echo "app exited before loading"; exit 1; }
        sleep 0.2
    done
    if [[ -n ${ACTIONS:-} ]]; then
        env -u LD_LIBRARY_PATH DISPLAY="$display" OUT="$out" APP_PID="$app_pid" CMD="$cmd_fifo" \
            bash "$ACTIONS"
    fi
    if ! kill -0 "$app_pid" 2>/dev/null; then
        finish
    fi
    DISPLAY=$display import -window root "$out/screenshot.png"
    echo "screenshot $out/screenshot.png"
    exit 0
fi

finish
