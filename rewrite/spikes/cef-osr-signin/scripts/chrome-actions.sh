#!/usr/bin/env bash
# Chrome-mode checks: a tab opened from Chrome's UI is tracked (default_client), closing the
# first tab keeps the app running, and closing the last tab exits it. A web page's console
# output is not relayed. Run through xvfb-isolated.sh with ACTIONS=<this file> on
# scripts/input-test.html, with WAIT_SECS and XDOTOOL set. Needs node for the loopback fixture
# server. The harness reports the app's exit status.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
start_fixture_server

wid=$(window '^Flecha spike input test')
xdo windowactivate --sync "$wid" 2>/dev/null || xdo windowfocus --sync "$wid"
wait_for '^\[console\] INPUT storage loads'
sleep 0.5

# New tab from Chrome's UI, then navigate it through the omnibox to the loopback marker page.
# Its /ran request proves the page loaded and logged its marker.
m=$(mark)
fm=$(fixture_mark)
xdo key ctrl+t
wait_for '^\[browser\] created id=[0-9]+ open=2' "$m"
sleep 0.5
xdo key ctrl+l
xdo type --delay 40 "$FIXTURE/marker?tab"
xdo key Return
wait_for "^\\[load-end\\] ${FIXTURE//./\\.}\$" "$m"
wait_fixture '^GET /ran\?tab$' "$fm"
sleep 0.5
import -window root "$OUT/chrome-two-tabs.png"

# Close the first tab: the app keeps running with one browser.
xdo key ctrl+1
sleep 0.5
xdo key ctrl+w
wait_for '^\[browser\] closed id=1 open=1' "$m"
sleep 0.5
check alive

# Close the last tab: the app exits. By now the marker page's console output had time to
# arrive; it must not have been relayed.
xdo key ctrl+w
wait_for '^\[browser\] closed id=[0-9]+ open=0' "$m"
wait_exit
check test "$(count 'FLECHA_CONSOLE_MARKER')" = 0
logged=$(debug_log_marker_count)
check test "$logged" = 0
