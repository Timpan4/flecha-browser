#!/usr/bin/env bash
# The --fake-camera guard: with the fake capture device on, the spike loads only file:// and
# loopback (127.0.0.1, localhost, [::1]) URLs, so the device can never be combined with a
# sign-in page. A second fixture server on 127.0.0.2 stands in for a remote origin (nothing
# leaves the host). From a loopback page the script tries a frame, an image, a fetch and a
# redirected fetch to it, then a top-level navigation and a redirected navigation; OSR also
# types `go https://accounts.google.com/` and `go https://mail.google.com/`, which must be
# refused before any request is made.
# Pass (GUARD=on, the spike started with --fake-camera): no request reached 127.0.0.2 and
# the spike logged [blocked] or [go] refused lines.
# The negative control (GUARD=off, same script without --fake-camera) must reach 127.0.0.2.
# Run through xvfb-isolated.sh with ACTIONS=<this file>, initial URL scripts/input-test.html.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
# shellcheck source=auto-lib.sh
. "$(dirname "$0")/auto-lib.sh"
: "${GUARD:?set GUARD to on (started with --fake-camera) or off}"
start_servers
app_window >/dev/null

fm=$(fixture_mark)
m=$(mark)
open_url "$FIXTURE/guard?remote=$REMOTE"
wait_fixture '^GET /done$' "$fm"
# The frame settles one way or the other: blocked, or its request reaches 127.0.0.2.
frame_settled() {
    local reached blocked
    reached=$(grep -ac '^GET /frame$' "$OUT/remote-server.log" || true)
    blocked=$(count '^\[blocked\] navigation to http://127\.0\.0\.2' "$m")
    ((reached + blocked > 0))
}
wait_until 'the frame settled' frame_settled || true

m=$(mark)
open_url "$REMOTE/navigate"
open_url "$FIXTURE/to?$REMOTE/navigate-redirected"
if [[ $GUARD == on && $APP_MODE == osr ]]; then
    send 'go https://accounts.google.com/'
    send 'go https://mail.google.com/'
    wait_for '^\[go\] refused: .*mail\.google\.com' "$m" >/dev/null
fi
# Each navigation either reaches 127.0.0.2 or is blocked; wait for that rather than a time.
navigated() {
    local reached blocked
    reached=$(grep -acE '^GET /navigate(-redirected)?$' "$OUT/remote-server.log" || true)
    blocked=$(count '^\[blocked\] (navigation|redirect|request) to http://127\.0\.0\.2' "$m")
    ((reached + blocked >= 2))
}
wait_until 'both navigations settled' navigated || true

reached=$(remote_requests)
blocked=$(count '^\[blocked\] ')
refused=$(count '^\[go\] refused: ')
echo "remote requests: $reached; [blocked] lines: $blocked; [go] refused lines: $refused"
sed 's/^/remote: /' "$OUT/remote-server.log"
if [[ $GUARD == on ]]; then
    if ((reached == 0 && blocked > 0)); then
        cell guard pass "auto, Xvfb, --fake-camera: 0 requests reached 127.0.0.2 (frame, image, fetch, redirected fetch, navigation, redirected navigation); $blocked [blocked] lines, $refused go commands refused"
    else
        cell guard fail "auto, Xvfb, --fake-camera: $reached requests reached 127.0.0.2; $blocked [blocked] lines"
    fi
else
    if ((reached > 0)); then
        cell guard-control pass "auto, Xvfb, no --fake-camera: $reached requests reached 127.0.0.2, so the guarded run's 0 is the guard's doing"
    else
        cell guard-control fail "auto, Xvfb, no --fake-camera: no request reached 127.0.0.2, so the guarded run proves nothing"
    fi
fi
