#!/usr/bin/env bash
# OSR checks beyond typing: window.open popup, permission prompts answered through the terminal,
# terminal commands (pdf, go, quit), and an AltGr character on a Swedish layout.
# Run through xvfb-isolated.sh with ACTIONS=<this file> on scripts/input-test.html, with
# WAIT_SECS and XDOTOOL set. The harness reports the app's exit status after `quit`.
set -euo pipefail
. "$(dirname "$0")/lib-actions.sh"

wid=$("$x" search --sync --name '^Flecha spike: osr$')
"$x" windowactivate --sync "$wid" 2>/dev/null || "$x" windowfocus --sync "$wid"
wait_for '^\[console\] INPUT storage loads'
sleep 0.5

# window.open: a second OSR window opens, paints, and has an opener.
"$x" mousemove --window "$wid" 170 335 click 1
wait_for '^\[popup\] '
wait_for '^\[console\] INPUT child opener true'
wait_for '^\[browser\] created id=[0-9]+ open=2'
pid=$("$x" search --sync --name '^Flecha spike: osr popup$')
sleep 0.5
import -window root "$OUT/osr-popup.png"
# window.close() from the popup closes only the popup.
"$x" mousemove --window "$pid" 170 35 click 1
wait_for '^\[browser\] closed id=[0-9]+ open=1'
check alive

# Notification permission: the prompt is printed, `allow N` answers it.
"$x" mousemove --window "$wid" 170 385 click 1
line=$(wait_for '^\[permission [0-9]+\] .*|^\[console\] INPUT notification')
echo "$line"
if [[ $line =~ ^\[permission\ ([0-9]+)\] ]]; then
    send "allow ${BASH_REMATCH[1]}"
fi
wait_for '^\[console\] INPUT notification'

# Camera: Xvfb has no camera, so this records whether a prompt appears before device lookup.
before=$(count '^\[permission [0-9]+\]')
"$x" mousemove --window "$wid" 170 435 click 1
line=$(wait_for '^\[console\] INPUT camera|^\[permission [0-9]+\] .*camera')
echo "$line"
if [[ $line =~ ^\[permission\ ([0-9]+)\] ]]; then
    send "deny ${BASH_REMATCH[1]}"
    wait_for '^\[console\] INPUT camera'
fi
echo "permission prompts for camera: $(( $(count '^\[permission [0-9]+\]') - before ))"

# AltGr on a Swedish layout: AltGr+2 is @.
DISPLAY=$DISPLAY setxkbmap se
"$x" mousemove --window "$wid" 170 35 click 1
"$x" keydown ISO_Level3_Shift key 2 keyup ISO_Level3_Shift
"$x" type --delay 40 'åäö'
wait_for '^\[console\] INPUT text ".*@'
wait_for '^\[console\] INPUT text ".*@åäö"'
DISPLAY=$DISPLAY setxkbmap us

# Terminal commands.
send "pdf $OUT/osr-print.pdf"
wait_for '^\[pdf\] ok=true'
check test -s "$OUT/osr-print.pdf"
send 'go https://www.google.com/'
wait_for '^\[load-end\] https://'
send 'quit'
wait_exit
