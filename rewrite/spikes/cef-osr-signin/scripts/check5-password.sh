#!/usr/bin/env bash
# Check 5, password save and fill, with --probe on the public demo login
# (https://the-internet.herokuapp.com/login, user tomsmith, password SuperSecretPassword!, as
# the page itself shows). auto-checks.sh always passes --password-store=basic and a scratch
# HOME, so no keyring is touched; Login Data is read read-only with sqlite3.
# PHASE=save: type the demo login, submit, wait for the secure page. Chrome and Flecha:
#   Alt+Shift+A focuses the save bubble and Return presses Save. OSR has no save prompt or
#   callback. Pass (5a): Login Data holds a login for the demo site.
# PHASE=fill: on a profile that holds the saved login, a real click on the page (Chromium
#   exposes autofilled values to the page only after a user gesture). Pass (5b): the probe
#   sees both fields non-empty.
# CONTROL=1: save without pressing Save, or fill on a profile with nothing saved; neither may
# pass.
# Run through xvfb-isolated.sh with ACTIONS=<this file>.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
# shellcheck source=auto-lib.sh
. "$(dirname "$0")/auto-lib.sh"
: "${PHASE:?set PHASE to save or fill}"
control=${CONTROL:-0}
store='--password-store=basic in a scratch HOME'

wid=$(app_window)
if [[ $PHASE == save ]]; then
    wait_until 'focus in the username field' probe_has login 0 '"focus":"username"'
    xdo type --delay 30 'tomsmith'
    xdo key Tab
    xdo type --delay 30 'SuperSecretPassword!'
    xdo key Return
    wait_until 'the secure page' probe_has secure 0 '"logged_in":true'
    if [[ $APP_MODE == osr ]]; then
        how='Alloy has no save prompt and no save callback to answer'
    elif ((control)); then
        how='save bubble left unanswered (control)'
    else
        how='Alt+Shift+A focused the save bubble, Return pressed Save'
        saved() {
            has_saved_login && return 0
            xdo key alt+shift+a Return
            return 1
        }
        wait_until 'a saved login' saved || true
    fi
    if wait_until 'a saved login' has_saved_login; then
        cell 5a pass "auto, Xvfb, $store: logged in with the public demo login; $how; Login Data holds $(saved_logins) login for the demo site"
    else
        cell 5a fail "auto, Xvfb, $store: logged in with the public demo login; $how; Login Data holds $(saved_logins) logins for the demo site after ${WAIT_SECS}s"
    fi
    exit 0
fi

wait_until 'a login report' probe_has login 0 || true
line=$(probe_last login)
before="before the click: user $(json_field "$line" user_len), password $(json_field "$line" pass_len) characters"
geometry=$(xdo getwindowgeometry --shell "$wid")
w=$(sed -n 's/^WIDTH=//p' <<<"$geometry") h=$(sed -n 's/^HEIGHT=//p' <<<"$geometry")
m=$(mark)
# A click on blank page area below the form: a user gesture that selects nothing.
xdo mousemove --window "$wid" "$((w / 2))" "$((h * 9 / 10))" click 1
if wait_until 'filled fields' fields_filled "$m"; then
    line=$(probe_last login "$m")
    cell 5b pass "auto, Xvfb, $store, $(saved_logins) saved login in the profile: after a real click the fields hold user $(json_field "$line" user_len) and password $(json_field "$line" pass_len) characters ($before)"
else
    cell 5b fail "auto, Xvfb, $store, $(saved_logins) saved logins in the profile: the fields stayed empty for ${WAIT_SECS}s after a real click ($before)"
fi
