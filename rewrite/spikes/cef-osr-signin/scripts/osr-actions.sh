#!/usr/bin/env bash
# OSR checks beyond typing: window.open popup, permission prompts answered through the
# terminal, console relay off for web pages, layouts (Swedish AltGr, French AZERTY), and the
# terminal commands (pdf, go, quit). Run through xvfb-isolated.sh with ACTIONS=<this file> on
# scripts/input-test.html, with WAIT_SECS and XDOTOOL set. Needs node for the loopback fixture
# server. The harness reports the app's exit status after `quit`.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
start_fixture_server

wid=$(window '^Flecha spike: osr$')
xdo windowactivate --sync "$wid" 2>/dev/null || xdo windowfocus --sync "$wid"
wait_for '^\[console\] INPUT storage loads'

# window.open: a second OSR window opens, paints, and has an opener.
m=$(mark)
xdo mousemove --window "$wid" 170 335 click 1
wait_for '^\[popup\] file://' "$m"
wait_for '^\[console\] INPUT child opener true' "$m"
wait_for '^\[browser\] created id=[0-9]+ open=2' "$m"
pid=$(window '^Flecha spike: osr popup$')
sleep 0.5
import -window root "$OUT/osr-popup.png"
# window.close() from the popup closes only the popup.
xdo mousemove --window "$pid" 170 35 click 1
wait_for '^\[browser\] closed id=[0-9]+ open=1' "$m"
check alive

# Notification permission: the prompt is printed, `allow N` grants it.
m=$(mark)
xdo mousemove --window "$wid" 170 385 click 1
line=$(wait_for '^\[permission [0-9]+\] ' "$m")
echo "$line"
[[ $line =~ ^\[permission\ ([0-9]+)\] ]]
send "allow ${BASH_REMATCH[1]}"
wait_for '^\[console\] INPUT notification granted' "$m"

# Camera: Xvfb has no camera. Records whether a prompt appears before the device lookup; a
# prompt is denied and must then be refused.
m=$(mark)
xdo mousemove --window "$wid" 170 435 click 1
line=$(wait_for '^\[console\] INPUT camera (granted|error)|^\[permission [0-9]+\] .*camera' "$m")
echo "$line"
if [[ $line =~ ^\[permission\ ([0-9]+)\] ]]; then
    send "deny ${BASH_REMATCH[1]}"
    wait_for '^\[console\] INPUT camera error NotAllowedError' "$m"
fi
echo "permission prompts for camera: $(count '^\[permission [0-9]+\]' "$m")"

# AltGr on a Swedish layout: AltGr+2 is @.
setxkbmap se
m=$(mark)
focus "$wid" text 170 35
xdo keydown ISO_Level3_Shift key 2 keyup ISO_Level3_Shift
xdo type --delay 40 'åäö'
wait_for '^\[console\] INPUT text "Hello Wörld 123!@åäö"|^\[console\] INPUT text "@åäö"' "$m"

# French AZERTY: the key labelled A sits at the physical Q position. Ctrl+A must reach the
# page as A (keyCode 65) with Ctrl held, so it selects all rather than acting as Ctrl+Q. A
# plain `a` would also be KeyQ 65, so the Ctrl flag is part of the assertion.
setxkbmap fr
m=$(mark)
xdo key ctrl+a
line=$(wait_for '^\[console\] INPUT keydown a ' "$m")
echo "$line"
setxkbmap us
[[ $line == *' KeyQ 65 ctrl true '* ]] || { echo "FAIL AZERTY Ctrl+A arrived as: $line" >&2; exit 1; }

# Terminal commands. A web page's console output is not relayed by default.
rm -f "$OUT/osr-print.pdf"
m=$(mark)
send "pdf $OUT/osr-print.pdf"
wait_for '^\[pdf\] ok=true' "$m"
check test -s "$OUT/osr-print.pdf"
m=$(mark)
fm=$(fixture_mark)
send "go $FIXTURE/marker?osr"
wait_for "^\\[load-end\\] ${FIXTURE//./\\.}\$" "$m"
wait_fixture '^GET /ran\?osr$' "$fm"
send "go file://$here/input-test.html"
wait_for '^\[console\] INPUT storage loads' "$m"
check test "$(count 'FLECHA_CONSOLE_MARKER' "$m")" = 0
send 'quit'
wait_exit
logged=$(debug_log_marker_count)
check test "$logged" = 0
