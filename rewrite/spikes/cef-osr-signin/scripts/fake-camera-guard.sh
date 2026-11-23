#!/usr/bin/env bash
# The --fake-camera guard: with the fake capture device on, the spike loads only file:// and
# loopback (127.0.0.1, localhost, [::1]) URLs, so the device can never be combined with a
# sign-in page. A second fixture server on 127.0.0.2 stands in for a remote origin (nothing
# leaves the host). From a loopback page the script tries a frame, an image, a fetch and a
# redirected fetch to it, then a top-level navigation and a redirected navigation; OSR also
# types `go https://accounts.google.com/` and `go https://mail.google.com/`, which must be
# refused before any request is made.
# Pass (GUARD=on, the spike started with --fake-camera): the frame and both navigations were
# logged as [blocked], OSR refused both `go` commands, and no request reached 127.0.0.2.
# The negative control (GUARD=off, same script without --fake-camera) passes only when all six
# requests (frame, image, fetch, redirected fetch, navigation, redirected navigation) reached
# 127.0.0.2.
# Run through xvfb-isolated.sh with ACTIONS=<this file>, initial URL scripts/input-test.html.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
# shellcheck source=auto-lib.sh
. "$(dirname "$0")/auto-lib.sh"
: "${GUARD:?set GUARD to on (started with --fake-camera) or off}"
start_servers
app_window >/dev/null
remote_log=$OUT/remote-server.log
remote_got() { grep -ac "^GET $1\$" "$remote_log" || true; }

fm=$(fixture_mark)
m=$(mark)
open_url "$FIXTURE/guard?remote=$REMOTE"
wait_fixture '^GET /done$' "$fm"
# The frame settles one way or the other: blocked, or its request reaches 127.0.0.2.
frame_settled() {
    local blocked
    blocked=$(count '^\[blocked\] navigation to http://127\.0\.0\.2' "$m")
    (($(remote_got /frame) + blocked > 0))
}
frame=no
wait_until 'the frame settled' frame_settled && frame=yes

m=$(mark)
open_url "$REMOTE/navigate"
open_url "$FIXTURE/to?$REMOTE/navigate-redirected"
refused=0
if [[ $GUARD == on && $APP_MODE == osr ]]; then
    send 'go https://accounts.google.com/'
    send 'go https://mail.google.com/'
    wait_for '^\[go\] refused: .*mail\.google\.com' "$m" >/dev/null
    refused=$(count '^\[go\] refused: .*(accounts|mail)\.google\.com' "$m")
fi
# Each navigation either reaches 127.0.0.2 or is blocked; wait for that rather than a time.
navigated() {
    local reached blocked
    reached=$(($(remote_got /navigate) + $(remote_got /navigate-redirected)))
    blocked=$(count '^(\[blocked\] (navigation|redirect) to|\[go\] refused: .*) http://127\.0\.0\.2' "$m")
    ((reached + blocked >= 2))
}
navigations=no
wait_until 'both navigations settled' navigated && navigations=yes

reached=$(remote_requests)
blocked=$(count '^\[blocked\] ')
echo "remote requests: $reached; [blocked] lines: $blocked; go commands refused: $refused"
sed 's/^/remote: /' "$remote_log"
if [[ $GUARD == on ]]; then
    osr_go=yes
    [[ $APP_MODE != osr ]] || ((refused == 2)) || osr_go=no
    if ((reached == 0)) && [[ $frame == yes && $navigations == yes && $osr_go == yes ]]; then
        cell guard pass "auto, Xvfb, --fake-camera: 0 requests reached 127.0.0.2 (frame, image, fetch, redirected fetch, navigation, redirected navigation all tried); $blocked [blocked] lines; $refused go commands to Google sign-in origins refused"
    else
        cell guard fail "auto, Xvfb, --fake-camera: $reached requests reached 127.0.0.2; frame settled: $frame; navigations settled: $navigations; $blocked [blocked] lines; $refused go commands refused"
    fi
else
    missing=''
    for path in /frame /img /fetch /redirected /navigate /navigate-redirected; do
        (($(remote_got "$path") > 0)) || missing+=" $path"
    done
    if [[ -z $missing ]]; then
        cell guard-control pass "auto, Xvfb, no --fake-camera: all six requests reached 127.0.0.2, so the guarded run's 0 is the guard's doing"
    else
        cell guard-control fail "auto, Xvfb, no --fake-camera: missing at 127.0.0.2:$missing, so the guarded run proves less"
    fi
fi
