#!/usr/bin/env bash
# Second run on a profile: prints the page's load count from localStorage and the profile's
# cookie hosts, then closes the window the way a desktop does. Xvfb has no window manager, so
# this starts $WM (an EWMH window manager such as openbox) on the test display; xdotool's
# windowquit then asks it to close the window, and it sends WM_DELETE_WINDOW.
# Run through xvfb-isolated.sh with ACTIONS=<this file> on scripts/input-test.html, with
# WAIT_SECS, XDOTOOL, WM and PROFILE set.
set -euo pipefail
. "$(dirname "$0")/lib-actions.sh"
: "${PROFILE:?set PROFILE to the --profile path}" "${WM:?set WM to a window manager binary}"

wid=$("$x" search --sync --onlyvisible --name '^Flecha spike')
wait_for '^\[console\] INPUT storage loads'
echo "cookie hosts at start: $(sqlite3 "file:$PROFILE/Default/Cookies?mode=ro&immutable=1" \
    'select group_concat(host_key) from (select distinct host_key from cookies)')"

HOME=$OUT/home XDG_CONFIG_HOME=$OUT/xdg-config XDG_CACHE_HOME=$OUT/xdg-cache \
    XDG_DATA_HOME=$OUT/xdg-data XDG_RUNTIME_DIR=$OUT/runtime \
    "$WM" >"$OUT/wm.log" 2>&1 </dev/null &
wm_pid=$!
trap 'kill "$wm_pid" 2>/dev/null || true; wait "$wm_pid" 2>/dev/null || true' EXIT
deadline=$((SECONDS + WAIT_SECS))
until xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -c 'window id' >/dev/null; do
    ((SECONDS < deadline)) || { echo "FAIL window manager did not start" >&2; exit 1; }
    sleep 0.2
done
"$x" windowquit "$wid"
wait_exit
