#!/usr/bin/env bash
# Check 0, typing and paste, on scripts/input-test.html. A Swedish keymap (setxkbmap se on the
# Xvfb display) types @, åäö, AltGr characters, € and a Shift capital; then Ctrl+V pastes from
# a clipboard owned by xclip on the same Xvfb display. Pass: the text field reports exactly
# the typed, then the typed and pasted, text.
# CONTROL=1 removes the cause: no click into the field (keys go to the document) and no
# clipboard owner, so neither cell may pass.
# Needs XCLIP (path to xclip) and node. Run through xvfb-isolated.sh with ACTIONS=<this file>.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
# shellcheck source=auto-lib.sh
. "$(dirname "$0")/auto-lib.sh"
: "${XCLIP:?set XCLIP to the xclip binary}"
control=${CONTROL:-0}

wid=$(app_window)
wait_for '^\[console\] INPUT storage loads' >/dev/null
setxkbmap -display "$DISPLAY" se

click_note=''
if [[ $control != 1 ]]; then
    m=$(mark)
    if [[ $APP_MODE == osr ]]; then
        xdo mousemove --window "$wid" 170 35 click 1
    else
        xdo key Tab
    fi
    if ! wait_for '^\[console\] INPUT focus text ' "$m" >/dev/null 2>&1; then
        if [[ $APP_MODE == osr ]]; then
            # Counted, not hidden: issue #94's lost first click.
            click_note='; the first click was lost, a second one landed'
            m=$(mark)
            xdo mousemove --window "$wid" 170 35 click 1
            wait_for '^\[console\] INPUT focus text ' "$m" >/dev/null 2>&1 ||
                click_note='; two clicks on the field were lost'
        else
            click_note='; Tab did not focus the field'
        fi
    fi
fi

m=$(mark)
typed='@åäö{[]}\€A'
xdo key ISO_Level3_Shift+2 aring adiaeresis odiaeresis ISO_Level3_Shift+7 ISO_Level3_Shift+8 \
    ISO_Level3_Shift+9 ISO_Level3_Shift+0 ISO_Level3_Shift+plus ISO_Level3_Shift+e shift+a
if wait_until 'typed text' typed_text_is "$typed" "$m"; then
    cell 0a pass "auto, Xvfb, xdotool with setxkbmap se: AltGr+2, å, ä, ö, AltGr+7/8/9/0/+, AltGr+e, Shift+A gave the field exactly $typed$click_note"
else
    cell 0a fail "auto, Xvfb, xdotool with setxkbmap se: the field did not report $typed$click_note"
fi

pasted='Klistra ÅÄÖ €42'
xclip_pid='' owner=none
if [[ $control != 1 ]]; then
    printf '%s' "$pasted" | "$XCLIP" -selection clipboard -i -quiet >/dev/null 2>&1 &
    xclip_pid=$! owner="xclip on the same display"
    trap 'kill "$xclip_pid" 2>/dev/null || true' EXIT
    wait_until 'xclip owns the clipboard' bash -c '[[ $("$1" -selection clipboard -o) == "$2" ]]' _ "$XCLIP" "$pasted"
fi
m=$(mark)
xdo key ctrl+v
if wait_until 'pasted text' typed_text_is "$typed$pasted" "$m"; then
    cell 0b pass "auto, Xvfb: Ctrl+V pasted text owned by xclip on the same display"
else
    cell 0b fail "auto, Xvfb: no pasted text reached the field within ${WAIT_SECS}s of Ctrl+V (clipboard owner: $owner)"
fi
