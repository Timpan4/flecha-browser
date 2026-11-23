# shellcheck shell=bash
# shellcheck disable=SC2154 # log, here and fixture_pid come from lib-actions.sh, sourced first
# Helpers for the auto-checks ACTIONS scripts (check*.sh), sourced after lib-actions.sh.
# APP_MODE is osr, chrome or flecha (today's Flecha, the reference column). Each script prints
# its results as CELL lines, which auto-checks.sh turns into the results table:
#   CELL<TAB><row><TAB><mode><TAB><result><TAB><note>
# A result is pass, fail, not automated, n/a or inconclusive. Every pass condition is a
# function below, so a negative control runs the same function on a run with the cause
# removed and must not pass.
: "${APP_MODE:?set APP_MODE to osr, chrome or flecha}"

DARK_READER_ID=eimadpbcbfnmbkopoojfekhnkhdbieeh
PRINT_PHRASE='Flecha print check: the quick brown fox'
profile=${PROFILE:-$OUT/profile}

cell() {
    local row=$1 result=$2
    shift 2
    printf 'CELL\t%s\t%s\t%s\t%s\n' "$row" "$APP_MODE" "$result" "$*"
}

# app_window: waits for the app's main window, gives it the X input focus and prints its id.
app_window() {
    local wid
    case $APP_MODE in
        osr) wid=$(window '^Flecha spike: osr$') ;;
        *) wid=$(window ' - (Chromium|Flecha)$') ;;
    esac
    xdo windowactivate --sync "$wid" 2>/dev/null || xdo windowfocus --sync "$wid"
    printf '%s\n' "$wid"
}

# open_url <url>: OSR through the terminal command, the others through the address bar
# (Ctrl+L, type, Enter: keyboard only).
open_url() {
    if [[ $APP_MODE == osr ]]; then
        send "go $1"
    else
        xdo key ctrl+l
        sleep 0.3
        xdo type --delay 20 "$1"
        xdo key Return
    fi
}

# wait_until <description> <command...>: runs the command until it succeeds or WAIT_SECS pass.
wait_until() {
    local what=$1 deadline=$((SECONDS + WAIT_SECS))
    shift
    until "$@" 2>/dev/null; do
        if ((SECONDS >= deadline)); then
            echo "waited ${WAIT_SECS}s for: $what" >&2
            return 1
        fi
        sleep 0.5
    done
}

# probe_last <page> [mark]: the last [probe] report for a page after line <mark>.
probe_last() {
    tail -n "+$((${2:-0} + 1))" "$log" | grep -a "^\\[probe\\] .*\"page\":\"$1\"" | tail -n 1 || true
}

# json_field <json line> <key>: a scalar field of a [probe] report.
json_field() { sed -n "s/.*\"$2\":\\(\"[^\"]*\"\\|[^,}]*\\).*/\\1/p" <<<"$1" | tr -d '"'; }

# --- pass conditions ---

# typed_text_is <expected> [mark]: the text field's last reported value equals <expected>.
typed_text_is() {
    local line
    line=$(tail -n "+$((${2:-0} + 1))" "$log" | grep -a '^\[console\] INPUT text ' | tail -n 1 || true)
    [[ $line == "[console] INPUT text $(printf '%s' "$1" | node -e 'process.stdout.write(JSON.stringify(require("fs").readFileSync(0, "utf8")))') "* ]]
}

# extension_installed: Dark Reader is unpacked in the profile.
extension_installed() {
    compgen -G "$profile/Default/Extensions/$DARK_READER_ID/*/manifest.json" >/dev/null
}

# mean_grey <png> <window id>: mean brightness, 0 to 255, of a strip of the window's page
# area away from text (x 85-95 %, y 50-90 % of the window).
mean_grey() {
    local geometry x y w h
    geometry=$(xdo getwindowgeometry --shell "$2")
    x=$(sed -n 's/^X=//p' <<<"$geometry") y=$(sed -n 's/^Y=//p' <<<"$geometry")
    w=$(sed -n 's/^WIDTH=//p' <<<"$geometry") h=$(sed -n 's/^HEIGHT=//p' <<<"$geometry")
    convert "$1" -crop "$((w / 10))x$((h * 4 / 10))+$((x + w * 85 / 100))+$((y + h / 2))" \
        -colorspace Gray -format '%[fx:round(255*mean)]' info:
}

# is_dark <png> <window id>: darker than mid-grey (the midpoint of the 8-bit range). A light
# page (white or #eee) is far above it, Dark Reader's default background (#181a1b) far below.
is_dark() { (($(mean_grey "$1" "$2") < 128)); }

# pdf_has_phrase <pdf>: the PDF exists and its text contains PRINT_PHRASE.
pdf_has_phrase() {
    local text
    [[ -s $1 ]] && text=$(pdftotext "$1" - 2>/dev/null) && [[ ${text//$'\n'/ } == *"$PRINT_PHRASE"* ]]
}

# saved_logins: how many logins the profile's password store holds for the demo site.
saved_logins() {
    local db=$profile/Default/Login\ Data
    if [[ ! -f $db ]]; then
        echo 0
        return
    fi
    sqlite3 "file:$db?mode=ro&immutable=1" \
        "select count(*) from logins where origin_url like 'https://the-internet.herokuapp.com/%'"
}

has_saved_login() { (($(saved_logins) > 0)); }

# fields_filled [mark]: the login probe saw both fields non-empty after line <mark>.
fields_filled() {
    local line
    line=$(probe_last login "${1:-0}")
    [[ -n $line && $(json_field "$line" user_len) -gt 0 && $(json_field "$line" pass_len) -gt 0 ]]
}

# camera_granted [mark]: the test page reported a granted camera with one video track.
camera_granted() {
    (($(count '^\[console\] INPUT camera granted 1 ' "${1:-0}") > 0))
}

# start_servers: the loopback fixture server (FIXTURE) and a second one on 127.0.0.2
# (REMOTE), which --fake-camera must treat as remote. Both stop when the script exits.
start_servers() {
    local port_file=$OUT/remote-port deadline=$((SECONDS + WAIT_SECS))
    start_fixture_server
    rm -f "$port_file"
    node "$here/fixture-server.mjs" "$port_file" 127.0.0.2 >"$OUT/remote-server.log" 2>&1 &
    remote_pid=$!
    trap 'kill "$fixture_pid" "$remote_pid" 2>/dev/null || true' EXIT
    until [[ -s $port_file ]]; do
        if ((SECONDS >= deadline)) || ! kill -0 "$remote_pid" 2>/dev/null; then
            echo "FAIL remote fixture server did not start; see $OUT/remote-server.log" >&2
            return 1
        fi
        sleep 0.2
    done
    REMOTE=http://127.0.0.2:$(<"$port_file")
    export REMOTE
}

# remote_requests: how many requests reached the 127.0.0.2 server.
remote_requests() { grep -ac '' "$OUT/remote-server.log" || true; }

# probe_has <page> <mark> <fragment>...: the last [probe] report for <page> after line <mark>
# contains every fragment, such as '"focused":true' (reports list their keys in sorted order,
# so fragments are matched one by one rather than as one pattern).
probe_has() {
    local line fragment
    line=$(probe_last "$1" "$2")
    shift 2
    [[ -n $line ]] || return 1
    for fragment; do
        [[ $line == *"$fragment"* ]] || return 1
    done
}
