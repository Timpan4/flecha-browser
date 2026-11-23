#!/usr/bin/env bash
# Chrome-mode checks: a tab opened from Chrome's UI is tracked (default_client), closing the
# first tab keeps the app running, and closing the last tab exits it.
# Run through xvfb-isolated.sh with ACTIONS=<this file> on scripts/input-test.html, with
# WAIT_SECS and XDOTOOL set. The harness reports the app's exit status.
set -euo pipefail
. "$(dirname "$0")/lib-actions.sh"

wid=$(xdo search --sync --onlyvisible --name '^Flecha spike input test')
xdo windowactivate --sync "$wid" 2>/dev/null || xdo windowfocus --sync "$wid"
wait_for '^\[console\] INPUT storage loads'
sleep 0.5

# New tab from Chrome's UI, then navigate it through the omnibox.
xdo key ctrl+t
wait_for '^\[browser\] created id=[0-9]+ open=2'
sleep 0.5
xdo key ctrl+l
xdo type --delay 40 'https://www.google.com/'
xdo key Return
wait_for '^\[load-end\] https://'
sleep 0.5
import -window root "$OUT/chrome-two-tabs.png"

# Close the first tab: the app keeps running with one browser.
xdo key ctrl+1
sleep 0.5
xdo key ctrl+w
wait_for '^\[browser\] closed id=1 open=1'
sleep 0.5
check alive

# Close the last tab: the app exits.
xdo key ctrl+w
wait_for '^\[browser\] closed id=[0-9]+ open=0'
wait_exit
