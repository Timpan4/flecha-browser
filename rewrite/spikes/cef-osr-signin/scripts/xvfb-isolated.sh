#!/usr/bin/env bash
# Runs the spike on its own Xvfb display, never on the desktop session.
#
#   xvfb-isolated.sh <out-dir> <cef-osr-signin args...>
#
# Isolation: a private D-Bus session bus without service activation (session-bus.conf),
# scratch HOME, XDG and runtime dirs under <out-dir>, no inherited LD_LIBRARY_PATH, and
# WAYLAND_DISPLAY unset. Logs go to <out-dir>/stdout.log and stderr.log. Xvfb and the app are
# stopped by PID.
#
# With SCREENSHOT=1 the script waits for the app's first "[load-end]" line, saves
# <out-dir>/screenshot.png from the Xvfb root window, then stops the app. With ACTIONS=<script>
# it runs that script (DISPLAY and OUT set) before the screenshot.
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
bin=${BIN:-$here/../../../target/debug/cef-osr-signin}
out=$1
shift
mkdir -p "$out"/{home,xdg-config,xdg-cache,xdg-data,runtime}
chmod 700 "$out/runtime"

fifo=$out/display.fifo
rm -f "$fifo"
mkfifo "$fifo"
Xvfb -displayfd 3 -nolisten tcp 3>"$fifo" 2>"$out/xvfb.log" &
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
    rm -f "$fifo"
}
trap cleanup EXIT
read -r display_number <"$fifo"
display=:$display_number
echo "xvfb pid $xvfb_pid display $display"

env -u WAYLAND_DISPLAY -u DBUS_SESSION_BUS_ADDRESS -u LD_LIBRARY_PATH \
    DISPLAY="$display" HOME="$out/home" XDG_RUNTIME_DIR="$out/runtime" \
    XDG_CONFIG_HOME="$out/xdg-config" XDG_CACHE_HOME="$out/xdg-cache" \
    XDG_DATA_HOME="$out/xdg-data" XDG_SESSION_TYPE=x11 \
    dbus-run-session --config-file="$here/session-bus.conf" -- "$bin" "$@" \
    </dev/null >"$out/stdout.log" 2>"$out/stderr.log" &
app_pid=$!
echo "app pid $app_pid"

if [[ ${SCREENSHOT:-0} == 1 || -n ${ACTIONS:-} ]]; then
    until grep -c '^\[load-end\] ' "$out/stderr.log" >/dev/null; do
        kill -0 "$app_pid" 2>/dev/null || { echo "app exited before loading"; exit 1; }
        sleep 0.2
    done
    if [[ -n ${ACTIONS:-} ]]; then
        env -u LD_LIBRARY_PATH DISPLAY="$display" OUT="$out" bash "$ACTIONS"
    fi
    DISPLAY=$display import -window root "$out/screenshot.png"
    echo "screenshot $out/screenshot.png"
    exit 0
fi

set +e
wait "$app_pid"
status=$?
set -e
app_pid=
echo "app exit $status"
exit "$status"
