#!/usr/bin/env bash
# Second run on a profile: the page's load counter from localStorage must have grown and the
# loopback fixture's cookie (set in the first run) must be in the profile's cookie store.
# Then the window is closed the way a desktop does. Xvfb has no window manager, so this starts
# $WM (an EWMH window manager such as openbox) on the test display; xdotool's windowquit asks
# it to close the window, and it sends WM_DELETE_WINDOW.
# Run through xvfb-isolated.sh with ACTIONS=<this file> on scripts/input-test.html, with
# WAIT_SECS, XDOTOOL, WM and PROFILE set.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
: "${PROFILE:?set PROFILE to the --profile path}" "${WM:?set WM to a window manager binary}"

wid=$(window '^Flecha spike')
line=$(wait_for '^\[console\] INPUT storage loads [0-9]+')
echo "$line"
[[ $line =~ loads\ ([0-9]+) ]]
check test "${BASH_REMATCH[1]}" -ge 2
hosts=$(sqlite3 "file:$PROFILE/Default/Cookies?mode=ro&immutable=1" \
    "select group_concat(host_key) from (select distinct host_key from cookies where name = 'flecha_fixture')")
echo "fixture cookie hosts: $hosts"
check test "$hosts" = 127.0.0.1

"$WM" >"$OUT/wm.log" 2>&1 </dev/null &
wm_pid=$!
trap 'kill "$wm_pid" 2>/dev/null || true; wait "$wm_pid" 2>/dev/null || true' EXIT
deadline=$((SECONDS + WAIT_SECS))
until xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -c 'window id' >/dev/null; do
    ((SECONDS < deadline)) || { echo "FAIL window manager did not start" >&2; exit 1; }
    sleep 0.2
done
xdo windowquit "$wid"
wait_exit
