#!/usr/bin/env bash
# Console relay. Each case first proves the loopback marker page loaded and ran (its /ran
# request reaches the fixture server after it logged its markers), then counts each marker in
# the app's log:
#   main   the marker page as the main frame;
#   frame  a local file:// page that embeds the marker page in an iframe.
# The marker page logs a plain line and a line whose source is spoofed to file:// with
# `//# sourceURL`, both naming the case. EXPECT_MARKER=0 for a default run (no marker line of
# any kind), 1 for a run with --log-console (each of the four lines once, which shows the
# check can see them). CEF's own chrome_debug.log must never contain a marker. OSR mode; run
# through xvfb-isolated.sh with ACTIONS=<this file>, WAIT_SECS and EXPECT_MARKER set.
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
    "<iframe src=\"$FIXTURE/marker?frame\"></iframe>" >"$host"
send "go file://$host"
wait_fixture '^GET /ran\?frame$' "$fm"

# A round trip through another local page, so console messages sent before /ran have
# reached the app.
send "go file://$here/input-test.html"
wait_for '^\[console\] INPUT storage loads' "$m"
for case in main frame; do
    for kind in plain spoof; do
        found=$(count "FLECHA_CONSOLE_MARKER $kind $case " "$m")
        echo "marker lines, $kind $case: $found"
        check test "$found" = "$EXPECT_MARKER"
    done
done
check test "$(count 'FLECHA_CONSOLE_MARKER' "$m")" = "$((EXPECT_MARKER * 4))"
send 'quit'
wait_exit
# CEF's own log in the profile must not record page console output, with or without
# --log-console (the app consumes every console message).
logged=$(debug_log_marker_count)
echo "marker lines in chrome_debug.log: $logged"
check test "$logged" = 0
