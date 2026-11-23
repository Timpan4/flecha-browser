#!/usr/bin/env bash
# HISTORICAL, DO NOT RERUN. This is the record of a review repro run on
# 2026-11-23 against commit d7ade4a. It takes paths as arguments and copies or
# deletes files at them, and its environment predates the current isolation.
# The exit below keeps it from running; the lines after it are what ran.
echo "historical record; do not rerun" >&2
exit 1
# Negative control: the d7ade4a environment (desktop XDG_RUNTIME_DIR, default system bus).
set -euo pipefail
C=$1 bin=$2 page=$3
exec {dfd}<> <(:)
Xvfb -displayfd "$dfd" -screen 0 1920x1200x24 -nolisten tcp >"$C/xvfb.log" 2>&1 &
xp=$!
trap 'kill $xp; wait $xp 2>/dev/null || true; rm -rf -- "$cache"' EXIT
read -r -t 20 -u "$dfd" dn
cache=$(mktemp -d -t cef-gpui-pump-cache.XXXXXX)
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET DISPLAY=:$dn XDG_SESSION_TYPE=x11 \
  dbus-run-session --config-file="$C/old-bus.conf" -- "$bin" --ozone-platform=x11 --password-store=basic \
  --pump-url="file://$page" --pump-out="$C/idle.json" --pump-cache-dir="$cache" \
  --pump-warmup-ms=1000 --pump-measure-ms=1000 --pump-step-timeout-ms=20000 \
  --pump-forbid-paths="$XDG_RUNTIME_DIR:/run/dbus/system_bus_socket" --pump-scenario=idle >/dev/null 2>"$C/idle.stderr" || echo "app exit $?"
