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

# Each field is clicked and its focus event awaited before typing.
# Text field, including a non-ASCII character.
focus "$wid" text 170 35
xdo type --delay 40 'Hello Wörld 123!'
wait_for '^\[console\] INPUT text "Hello Wörld 123!"'

# Password field, then Enter. The page reports only the length and whether it matches.
focus "$wid" password 170 85
xdo type --delay 40 'pa$$ wörd'
wait_for '^\[console\] INPUT password length 9 scripted true'
xdo key Return; pause

# Textarea: Enter inserts a newline, Ctrl+A then Backspace clears it.
focus "$wid" area 170 160
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

# Ctrl+P calls BrowserHost::print(). It must reach CEF's print pipeline (the app's print
# handler logs OnPrintStart; with no printer settings Chromium then reports that no printer
# is available), and the app must survive it.
m=$(mark)
xdo key ctrl+p
wait_for '^\[print\] start' "$m"
check alive
