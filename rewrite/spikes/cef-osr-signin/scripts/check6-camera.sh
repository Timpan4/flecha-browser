#!/usr/bin/env bash
# Check 6, camera through a host prompt, on scripts/input-test.html. Xvfb has no camera, so
# auto-checks.sh starts the spike with --fake-camera (Chromium's fake capture device; the
# spike then loads only file:// and loopback pages, see src/common.rs). The page's camera
# button calls getUserMedia.
# OSR: a click on the button must print a [permission N] prompt; `allow N` answers it.
# Chrome and Flecha: Tab is pressed until the page reports focus on the camera button (a fixed
# count of Tabs can lose one to the browser's own UI), then Space presses it. Chrome's permission bubble
# opens with keyboard focus, which blurs the page (the page logs "window blur"); in the bubble
# Tab moves through Close, the device list and "Allow while visiting the site" (seen on
# CEF 158 and Chromium 158), and Return presses that. The bubble ignores input that arrives
# too soon after it is shown (Chromium's InputEventActivationProtector, against clickjacking),
# so Return is repeated until the page reports a result. Keyboard only: no clicks on Chrome's
# own UI.
# Pass: the page reports one granted video track.
# CONTROL=1 is the same script on a run without --fake-camera: no device, so no grant.
# Run through xvfb-isolated.sh with ACTIONS=<this file>.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
# shellcheck source=auto-lib.sh
. "$(dirname "$0")/auto-lib.sh"
control=${CONTROL:-0}
device=$( ((control)) && echo 'no camera device' || echo 'fake camera device')

wid=$(app_window)
wait_for '^\[console\] INPUT storage loads' >/dev/null
m=$(mark)
if [[ $APP_MODE == osr ]]; then
    xdo mousemove --window "$wid" 170 435 click 1
    line=$(wait_for '^\[console\] INPUT camera (granted|error)|^\[permission [0-9]+\] ' "$m" 2>/dev/null || true)
    prompt=none
    if [[ $line =~ ^\[permission\ ([0-9]+)\] ]]; then
        prompt="[permission ${BASH_REMATCH[1]}] answered with allow ${BASH_REMATCH[1]}"
        send "allow ${BASH_REMATCH[1]}"
    fi
    how="auto, Xvfb, $device: prompt: $prompt"
else
    # One Tab per step; the step ends once the latest focus the page logged is the camera button.
    camera_focused() {
        local last
        last=$(tail -n "+$((m + 1))" "$log" | grep -a '^\[console\] INPUT focus ' | tail -n 1 || true)
        [[ $last == '[console] INPUT focus camera '* ]] && return 0
        xdo key Tab
        return 1
    }
    if ! wait_until 'focus on the camera button' camera_focused; then
        cell 6 fail "auto, Xvfb, keyboard only, $device: Tab never reached the camera button"
        exit 0
    fi
    xdo key space
    wait_for '^\[console\] INPUT camera requested' "$m" >/dev/null || true
    if wait_for '^\[console\] INPUT window blur' "$m" >/dev/null 2>&1; then
        prompt='the permission bubble took focus; Tab Tab Tab, Return on "Allow while visiting the site"'
        xdo key Tab Tab Tab
        answer() {
            (($(count '^\[console\] INPUT camera (granted|error)' "$m") > 0)) && return 0
            xdo key Return
            return 1
        }
        wait_until 'a camera result' answer || true
    else
        prompt='no prompt took focus'
    fi
    how="auto, Xvfb, keyboard only, $device: Tab to the button, Space; $prompt"
fi
if wait_until 'a granted camera' camera_granted "$m"; then
    cell 6 pass "$how; the page got one video track"
else
    result=$(tail -n "+$((m + 1))" "$log" | grep -aoE '^\[console\] INPUT camera [a-z]+ [A-Za-z0-9]+' | tail -n 1 || true)
    cell 6 fail "$how; page result: ${result#\[console\] INPUT }"
fi
