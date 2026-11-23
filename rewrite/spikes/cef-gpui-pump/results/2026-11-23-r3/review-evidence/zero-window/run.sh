#!/usr/bin/env bash
# Old binary (d7ade4a) with --pump-measure-ms=0, in the new isolated environment.
set -euo pipefail
Z=$1 old=$2 B=$3 page=$4 conf=$5
cp "$old" "$B/cef-gpui-pump-old"
exec {dfd}<> <(:)
Xvfb -displayfd "$dfd" -screen 0 1920x1200x24 -nolisten tcp >"$Z/xvfb.log" 2>&1 &
xp=$!
cache=$(mktemp -d -t cef-gpui-pump-cache.XXXXXX); rt=$(mktemp -d -t cef-gpui-pump-runtime.XXXXXX)
trap 'kill $xp; wait $xp 2>/dev/null || true; rm -rf -- "$cache" "$rt"; rm -f "$B/cef-gpui-pump-old"' EXIT
read -r -t 20 -u "$dfd" dn
env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u DBUS_SESSION_BUS_ADDRESS DISPLAY=:$dn XDG_SESSION_TYPE=x11 \
  XDG_RUNTIME_DIR="$rt" DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent \
  dbus-run-session --config-file="$conf" -- "$B/cef-gpui-pump-old" --ozone-platform=x11 --password-store=basic \
  --pump-url="file://$page" --pump-out="$Z/idle.json" --pump-cache-dir="$cache" \
  --pump-warmup-ms=1000 --pump-measure-ms=0 --pump-step-timeout-ms=20000 --pump-scenario=idle \
  >/dev/null 2>"$Z/idle.stderr" || echo "app exit $?"
