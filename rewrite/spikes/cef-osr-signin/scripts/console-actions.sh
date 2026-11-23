#!/usr/bin/env bash
# Console relay: loads the loopback page that logs FLECHA_CONSOLE_MARKER, then a local
# fixture, and counts the marker in the log. EXPECT_MARKER=0 for a default run, 1 for a run
# with --log-console (which shows the check can see the marker). OSR mode; run through
# xvfb-isolated.sh with ACTIONS=<this file>, WAIT_SECS and EXPECT_MARKER set.
set -euo pipefail
# shellcheck source=lib-actions.sh
. "$(dirname "$0")/lib-actions.sh"
: "${EXPECT_MARKER:?set EXPECT_MARKER to 0 or 1}"
start_fixture_server

m=$(mark)
send "go $FIXTURE/marker"
wait_for "^\\[load-end\\] ${FIXTURE//./\\.}\$" "$m"
send "go file://$here/input-test.html"
wait_for '^\[console\] INPUT storage loads' "$m"
found=$(count 'FLECHA_CONSOLE_MARKER' "$m")
echo "marker lines: $found"
check test "$((found > 0 ? 1 : 0))" = "$EXPECT_MARKER"
send 'quit'
wait_exit
