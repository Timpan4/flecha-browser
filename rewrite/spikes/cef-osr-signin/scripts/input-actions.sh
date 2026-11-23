#!/usr/bin/env bash
# Drives scripts/input-test.html in an OSR window on Xvfb with synthetic X input.
# Run through xvfb-isolated.sh with ACTIONS=<this file> and XDOTOOL=<path to xdotool>.
# The pauses only give the page time to react between steps; they are not product limits.
set -euo pipefail
x=${XDOTOOL:-xdotool}
pause() { sleep 0.5; }

wid=$(xdo search --sync --name '^Flecha spike: osr$')
xdo windowactivate --sync "$wid" 2>/dev/null || xdo windowfocus --sync "$wid"
pause

# Text field, including a non-ASCII character.
xdo mousemove --window "$wid" 170 35 click 1; pause
xdo type --delay 40 'Hello Wörld 123!'; pause

# Password field, then Enter.
xdo mousemove --window "$wid" 170 85 click 1; pause
xdo type --delay 40 'pa$$ wörd'; pause
xdo key Return; pause

# Textarea: Enter inserts a newline, Ctrl+A then Backspace clears it.
xdo mousemove --window "$wid" 170 160 click 1; pause
xdo type --delay 40 'line1'; xdo key Return; xdo type --delay 40 'line2'; pause
xdo key ctrl+a; xdo key BackSpace; pause
xdo type --delay 40 'kept'; pause

# Double click on the button.
xdo mousemove --window "$wid" 170 285 click --repeat 2 --delay 80 1; pause

# <select> popup (PET_POPUP), choose the next option with the keyboard.
xdo mousemove --window "$wid" 170 235 click 1; pause
import -window root "$OUT/select-popup.png"
xdo key Down; xdo key Return; pause

# Wheel scrolling.
xdo mousemove --window "$wid" 600 400; xdo click --repeat 3 --delay 80 5; pause

# Ctrl+P calls BrowserHost::print(). Record whether the app survives it.
xdo key ctrl+p; pause; pause
