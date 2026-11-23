#!/usr/bin/env bash
# Chrome-mode checks: a tab opened from Chrome's UI is tracked (default_client), closing the
# first tab keeps the app running, and closing the last tab exits it.
# Run through xvfb-isolated.sh with ACTIONS=<this file> on scripts/input-test.html, with
# WAIT_SECS and XDOTOOL set. The harness reports the app's exit status.
set -euo pipefail
. "$(dirname "$0")/lib-actions.sh"

wid=$("$x" search --sync --onlyvisible --name '^Flecha spike input test')
"$x" windowactivate --sync "$wid" 2>/dev/null || "$x" windowfocus --sync "$wid"
wait_for '^\[console\] INPUT storage loads'
sleep 0.5

# New tab from Chrome's UI, then navigate it through the omnibox.
"$x" key ctrl+t
wait_for '^\[browser\] created id=[0-9]+ open=2'
sleep 0.5
"$x" key ctrl+l
"$x" type --delay 40 'https://www.google.com/'
"$x" key Return
wait_for '^\[load-end\] https://'
sleep 0.5
import -window root "$OUT/chrome-two-tabs.png"

# Close the first tab: the app keeps running with one browser.
"$x" key ctrl+1
sleep 0.5
"$x" key ctrl+w
wait_for '^\[browser\] closed id=1 open=1'
sleep 0.5
check alive

# Close the last tab: the app exits.
"$x" key ctrl+w
wait_for '^\[browser\] closed id=[0-9]+ open=0'
wait_exit
