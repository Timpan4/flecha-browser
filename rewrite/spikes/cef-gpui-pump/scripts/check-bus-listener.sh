#!/usr/bin/env bash
# Checks that the private bus config never replaces an existing socket: starts
# one bus in a scratch dir, starts a second bus on the same dir, and requires the
# first bus's socket to be the same file and still answering after the second exits.
# Usage: scripts/check-bus-listener.sh   (exits 0 when the listener is safe)
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
# Under /tmp, not the caller's TMPDIR, so the path the config and the bus
# address embed holds only characters sed, XML and D-Bus take literally.
dir=$(mktemp -d -p /tmp cef-gpui-pump-bus-check.XXXXXX)
trap 'rm -rf -- "$dir"' EXIT
[[ $dir =~ ^/tmp/cef-gpui-pump-bus-check\.[A-Za-z0-9]+$ ]] || { echo "unexpected scratch path $dir" >&2; exit 1; }
sed "s|__SCRATCH_RUNTIME_DIR__|$dir|" "$here/isolated-session-bus.conf.in" >"$dir/session-bus.conf"

# Runs inside the first bus's session.
inner() {
    local dir=$1 first before after
    first=${DBUS_SESSION_BUS_ADDRESS#unix:path=}
    first=${first%%,*}
    before=$(stat -c %i "$first")
    dbus-run-session --config-file="$dir/session-bus.conf" -- sh -c '
        second=${DBUS_SESSION_BUS_ADDRESS#unix:path=}; second=${second%%,*}
        [ "$second" != "$1" ] || { echo "second bus reused $1" >&2; exit 1; }' sh "$first"
    after=$(stat -c %i "$first" 2>/dev/null) || { echo "first bus socket $first is gone" >&2; exit 1; }
    [[ $before == "$after" ]] || { echo "first bus socket $first was replaced" >&2; exit 1; }
    dbus-send --session --dest=org.freedesktop.DBus --print-reply \
        /org/freedesktop/DBus org.freedesktop.DBus.GetId >/dev/null
    echo "bus listener ok: second bus used its own socket; $first unchanged and answering"
}
export -f inner
dbus-run-session --config-file="$dir/session-bus.conf" -- bash -c 'set -euo pipefail; inner "$1"' bash "$dir"
