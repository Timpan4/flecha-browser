#!/usr/bin/env bash
set -euo pipefail

backend=$1
cef_runtime=$2
cef_sha1=$3
shift 3
export HOME=/mnt/scratch/home XDG_CONFIG_HOME=/mnt/scratch/config XDG_CACHE_HOME=/mnt/scratch/cache
export XDG_DATA_HOME=/mnt/scratch/data XDG_STATE_HOME=/mnt/scratch/state XDG_RUNTIME_DIR=/mnt/scratch/runtime
export TMPDIR=/mnt/scratch/tmp XDG_SESSION_TYPE="$backend"
export DBUS_SESSION_BUS_ADDRESS=unix:path=/mnt/scratch/runtime/bus
export DBUS_SYSTEM_BUS_ADDRESS=unix:path=/mnt/scratch/runtime/no-system-bus
export FLECHA_PROFILE=/mnt/scratch/profile FLECHA_LAUNCHER=isolated
umask 077
mkdir -p "$HOME" "$XDG_RUNTIME_DIR" "$TMPDIR" "$FLECHA_PROFILE"

dbus-daemon --config-file=/mnt/work/tools/session-bus.conf --address="$DBUS_SESSION_BUS_ADDRESS" \
  --nofork --print-address > /mnt/scratch/bus.address 2> /mnt/scratch/bus.log &
bus=$!

case "$backend" in
  x11)
    mkdir -m 1777 /tmp/.X11-unix
    # Xvfb chooses the display number and publishes it only when it is ready.
    Xvfb -displayfd 3 -nolisten tcp 3>/mnt/scratch/display > /mnt/scratch/display.log 2>&1 &
    compositor=$!
    until [[ -s /mnt/scratch/display ]]; do kill -0 "$compositor"; sleep 0.1; done
    read -r display < /mnt/scratch/display
    export DISPLAY=":$display"
    ;;
  wayland)
    export WAYLAND_DISPLAY=flecha-isolated
    weston --backend=headless --renderer=vulkan --no-config --debug --fake-seat \
      --socket="$WAYLAND_DISPLAY" --log=/mnt/scratch/display.log &
    compositor=$!
    until [[ -S $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY ]]; do kill -0 "$compositor"; sleep 0.1; done
    ;;
  *) exit 2 ;;
esac

until [[ -s /mnt/scratch/bus.address ]]; do kill -0 "$bus"; sleep 0.1; done
if dbus-send --bus="$DBUS_SESSION_BUS_ADDRESS" --print-reply \
    --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.StartServiceByName \
    string:org.freedesktop.secrets uint32:0 > /mnt/scratch/activation.log 2>&1 ||
  ! grep -c 'AccessDenied' /mnt/scratch/activation.log >/dev/null; then
  printf 'Private bus did not reject keyring activation\n' >&2
  exit 1
fi
if [[ $backend == x11 ]]; then
  display_env=(DISPLAY="$DISPLAY")
else
  display_env=(WAYLAND_DISPLAY="$WAYLAND_DISPLAY")
fi
exec env -i PATH=/mnt/bin:/usr/bin:/bin LANG=C.UTF-8 \
  HOME="$HOME" TMPDIR="$TMPDIR" XDG_CONFIG_HOME="$XDG_CONFIG_HOME" \
  XDG_CACHE_HOME="$XDG_CACHE_HOME" XDG_RUNTIME_DIR="$XDG_RUNTIME_DIR" \
  XDG_DATA_HOME="$XDG_DATA_HOME" XDG_STATE_HOME="$XDG_STATE_HOME" \
  XDG_SESSION_TYPE="$XDG_SESSION_TYPE" "${display_env[@]}" \
  DBUS_SESSION_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS" DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SYSTEM_BUS_ADDRESS" \
  FLECHA_PROFILE="$FLECHA_PROFILE" FLECHA_LAUNCHER="$FLECHA_LAUNCHER" \
  FLECHA_CEF_RUNTIME_DIR="$cef_runtime" FLECHA_CEF_RUNTIME_SHA1="$cef_sha1" \
  LD_LIBRARY_PATH="$cef_runtime" "$@"
