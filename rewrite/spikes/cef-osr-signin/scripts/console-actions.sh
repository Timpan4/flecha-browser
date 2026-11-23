#!/usr/bin/env bash
# Console relay. Each case first proves the loopback marker page loaded and ran (its /ran
# request reaches the fixture server after it logged FLECHA_CONSOLE_MARKER), then counts the
# marker in the app's log:
#   1. the marker page as the main frame;
#   2. a local file:// page that embeds the marker page in an iframe, whose own console line
#      must be relayed while the iframe's must not.
# EXPECT_MARKER=0 for a default run (no marker line), 1 for a run with --log-console (both
# marker lines, which shows the check can see them). CEF's own chrome_debug.log must never
# contain the marker. OSR mode; run through xvfb-isolated.sh with ACTIONS=<this file>,
# WAIT_SECS and EXPECT_MARKER set.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
: "${EXPECT_MARKER:?set EXPECT_MARKER to 0 or 1}"
start_fixture_server

m=$(mark)
fm=$(fixture_mark)
send "go $FIXTURE/marker?main"
wait_for "^\\[load-end\\] ${FIXTURE//./\\.}\$" "$m"
wait_fixture '^GET /ran\?main$' "$fm"

host=$OUT/iframe-host.html
printf '%s\n' '<!doctype html><title>iframe host</title>' \
    "<iframe src=\"$FIXTURE/marker?frame\"></iframe>" \
    "<script>console.log('FLECHA_LOCAL_HOST')</script>" >"$host"
send "go file://$host"
wait_fixture '^GET /ran\?frame$' "$fm"
wait_for '^\[console\] FLECHA_LOCAL_HOST ' "$m"

# A round trip through another local page, so console messages sent before /ran have
# reached the app.
send "go file://$here/input-test.html"
wait_for '^\[console\] INPUT storage loads' "$m"
found=$(count 'FLECHA_CONSOLE_MARKER' "$m")
echo "marker lines: $found"
check test "$found" = "$((EXPECT_MARKER * 2))"
send 'quit'
wait_exit
# CEF's own log in the profile must not record page console output, with or without
# --log-console (the app consumes every console message).
logged=$(debug_log_marker_count)
echo "marker lines in chrome_debug.log: $logged"
check test "$logged" = 0
