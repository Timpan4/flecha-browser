#!/usr/bin/env bash
# Check 4, Web Store, with --probe on Dark Reader's store page. Nothing is signed in.
# 4a: the probe reports whether "Add to Chrome" is enabled, keeps focus on it and reports its
#     centre. OSR: a real click on that centre. Chrome: Return presses it; the install dialog
#     then takes focus from the page with Cancel focused, so Tab moves to "Add extension" and
#     Return accepts (keyboard only, no clicks on Chrome's own UI). A dialog can ignore keys
#     that arrive right after it opens, which ends in Cancel; the button then has focus again
#     and the sequence repeats until WAIT_SECS. Pass: Dark Reader is unpacked in the profile.
# 4b: open https://example.com/; pass when the probe reports the page loaded and a screenshot
#     of the page area is dark (is_dark). PHASE=content runs only this step, on a profile that
#     already holds Dark Reader (auto-checks.sh copies the Chrome-style one for OSR).
# 4c: OSR has no toolbar, so no surface for an extension popup; Chrome's popup needs the
#     toolbar's extension menu, which is Chrome's own UI: not automated.
# CONTROL=1: the same page, nothing pressed. Neither 4a nor 4b may pass.
# Run through xvfb-isolated.sh with ACTIONS=<this file>.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
# shellcheck source=auto-lib.sh
. "$(dirname "$0")/auto-lib.sh"
control=${CONTROL:-0}
phase=${PHASE:-install}

# install: step 4a. Prints its CELL line; returns 1 when the app is gone.
install() {
    local line button enabled store how='nothing pressed (control)' m
    wait_until 'an enabled store button' probe_has store 0 '"enabled":true' || true
    line=$(probe_last store)
    button=$(json_field "$line" button) enabled=$(json_field "$line" enabled)
    store="probe: button ${button:-not found}, enabled ${enabled:-unknown}"
    if ((!control)) && [[ $button == add && $enabled == true ]]; then
        m=$(mark)
        if [[ $APP_MODE == osr ]]; then
            # The button moves while the page lays out; take its centre from the latest report.
            wait_until 'the focused store button' probe_has store 0 '"focused":true' || true
            line=$(probe_last store)
            m=$(mark)
            xdo mousemove --window "$wid" "$(json_field "$line" x)" "$(json_field "$line" y)" click 1
            local activated=no
            wait_until 'a trusted click on the button' probe_has store "$m" '"activated":true' &&
                activated=yes
            how="real click on the button (the probe reported the trusted click: $activated)"
        else
            presses=0 dialogs=0 state=page
            wait_until 'Dark Reader installed' keyboard_install || true
            how="Return on the focused button ($presses presses), install dialog took focus $dialogs times, Tab and Return on it"
        fi
    fi
    if wait_until 'Dark Reader installed' extension_installed; then
        cell 4a pass "auto, Xvfb, not signed in, $store; $how; Dark Reader unpacked in the profile"
        # Chrome opens Dark Reader's welcome page in a new tab after the install; wait for it
        # so the address bar keys below go to a settled window.
        [[ $APP_MODE == osr ]] ||
            wait_for '^\[load-end\] https://darkreader\.org' >/dev/null 2>&1 || true
    elif ! alive; then
        cell 4a fail "auto, Xvfb, not signed in, $store; $how; the browser process died with no message on stderr (REPORT has the core dump's stack)"
        return 1
    elif ((!control)) && [[ $APP_MODE == osr && $how == *'click: no)' ]]; then
        cell 4a inconclusive "auto, Xvfb, not signed in, $store; $how; the click never reached the button, so the install was not tried"
    else
        cell 4a fail "auto, Xvfb, not signed in, $store; $how; no Dark Reader in the profile within ${WAIT_SECS}s"
    fi
}

# keyboard_install: one step of the Chrome-style install, run until Dark Reader is unpacked.
keyboard_install() {
    local line
    extension_installed && return 0
    line=$(probe_last store "$m")
    [[ -n $line ]] || line=$(probe_last store)
    case $state:$line in
        page:*'"page_focus":false'*)
            state=dialog dialogs=$((dialogs + 1))
            ;;
        page:*'"focused":true'*)
            xdo key Return
            presses=$((presses + 1))
            ;;
        dialog:*'"page_focus":false'*)
            xdo key Tab
            state=accept
            ;;
        accept:*'"page_focus":false'*)
            xdo key Return
            ;;
        *:*'"page_focus":true'*)
            state=page
            ;;
    esac
    return 1
}

wid=$(app_window)
if [[ $phase == install ]] && ! install; then
    cell 4b 'not reached' "the app died in 4a; see the next run for 4b"
    [[ $APP_MODE != osr ]] || cell 4c 'n/a' "auto, Xvfb: Alloy OSR has no toolbar or other surface to open an extension popup"
    exit 0
fi

if [[ $phase != install ]] && ! extension_installed; then
    cell 4b inconclusive "the copied profile holds no Dark Reader, so the content script cannot run"
    exit 0
fi

m=$(mark)
open_url https://example.com/
shot=$OUT/example.png
loaded=no
wait_until 'example.com loaded' probe_has example "$m" && loaded=yes
# Dark: example.com loaded (probe), its computed background is dark, the window shows it
# (Chrome-style: the title of the active tab; OSR has one page per window), and the screenshot
# of the page area is dark.
dark_now() {
    local colours
    colours=$(probe_last example "$m")
    [[ $loaded == yes ]] || return 1
    colour_dark "$(json_field "$colours" html)" || colour_dark "$(json_field "$colours" body)" || return 1
    [[ $APP_MODE == osr || $(xdo getwindowname "$wid") == 'Example Domain - '* ]] || return 1
    import -window root "$shot"
    is_dark "$shot" "$wid"
}
result=fail
wait_until 'a dark example.com' dark_now && result=pass
colours=$(probe_last example "$m")
label=''
[[ $phase == install ]] || label='on a copy of the Chrome-style profile with Dark Reader installed; '
[[ -s $shot ]] || import -window root "$shot"
cell 4b "$result" "auto, Xvfb screenshot, ${label}example.com loaded: $loaded; window title: $(xdo getwindowname "$wid"); page background grey level $(mean_grey "$shot" "$wid") of 255 (dark below 128); probe: body $(json_field "$colours" body), html $(json_field "$colours" html)"

if ((control)) || [[ $phase != install ]]; then
    exit 0
elif [[ $APP_MODE == osr ]]; then
    cell 4c 'n/a' "auto, Xvfb: Alloy OSR has no toolbar or other surface to open an extension popup"
else
    cell 4c 'not automated' "the popup opens from the toolbar's extension menu, Chrome's own UI"
fi
