#!/usr/bin/env bash
# Drives scripts/input-test.html in an OSR window on Xvfb with synthetic X input and asserts
# what the page received. Run through xvfb-isolated.sh with ACTIONS=<this file>, WAIT_SECS
# and XDOTOOL set. The short pauses only give the page time to react between steps; they
# are not product limits.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
pause() { sleep 0.5; }

wid=$(window '^Flecha spike: osr$')
xdo windowactivate --sync "$wid" 2>/dev/null || xdo windowfocus --sync "$wid"
wait_for '^\[console\] INPUT storage loads'

# Text field, including a non-ASCII character.
xdo mousemove --window "$wid" 170 35 click 1; pause
xdo type --delay 40 'Hello Wörld 123!'
wait_for '^\[console\] INPUT text "Hello Wörld 123!"'

# Password field, then Enter. The page reports only the length and whether it matches.
xdo mousemove --window "$wid" 170 85 click 1; pause
xdo type --delay 40 'pa$$ wörd'
wait_for '^\[console\] INPUT password length 9 scripted true'
xdo key Return; pause

# Textarea: Enter inserts a newline, Ctrl+A then Backspace clears it.
xdo mousemove --window "$wid" 170 160 click 1; pause
xdo type --delay 40 'line1'; xdo key Return; xdo type --delay 40 'line2'
wait_for '^\[console\] INPUT area "line1\\nline2"'
m=$(mark)
xdo key ctrl+a; xdo key BackSpace
wait_for '^\[console\] INPUT area ""' "$m"
xdo type --delay 40 'kept'
wait_for '^\[console\] INPUT area "kept"' "$m"

# Double click on the button.
m=$(mark)
xdo mousemove --window "$wid" 170 285 click --repeat 2 --delay 80 1
wait_for '^\[console\] INPUT click 2' "$m"
wait_for '^\[console\] INPUT dblclick' "$m"

# <select> popup (PET_POPUP), choose the next option with the keyboard.
xdo mousemove --window "$wid" 170 235 click 1; pause
import -window root "$OUT/select-popup.png"
xdo key Down; xdo key Return
wait_for '^\[console\] INPUT choice two'

# Wheel scrolling.
xdo mousemove --window "$wid" 600 400; xdo click --repeat 3 --delay 80 5
wait_for '^\[console\] INPUT scroll [1-9]'

# Ctrl+P calls BrowserHost::print(). The app must survive it.
xdo key ctrl+p; pause; pause
check alive
