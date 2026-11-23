#!/usr/bin/env bash
set -euo pipefail

spike=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
rewrite=$(cd "$spike/../.." && pwd)
backend=${1:?Usage: verify-linux.sh wayland|x11 1|2 auto|software|fallback [input|clipboard]}
scale=${2:?Set the isolated display scale}
mode=${3:?Set the expected OSR mode}
input=${4:-}
if [[ $scale != 1 && $scale != 2 ]]; then
  printf 'Select a 1x or 2x verification run.\n' >&2
  exit 2
fi
case "$mode" in
  auto|fallback) args=(--control) ;;
  software) args=(--control --software) ;;
  *) exit 2 ;;
esac
if [[ $backend == x11 && $mode == fallback ]]; then
  printf 'The import-failure verification needs the accelerated Wayland path.\n' >&2
  exit 2
fi
if [[ -n $input && ( ( $input != input && $input != clipboard ) || $backend != x11 || $scale != 1 ) ]]; then
  printf 'The input probe uses Xvfb at 1x.\n' >&2
  exit 2
fi
if [[ ! -v FLECHA_SPIKE_WATCHDOG ]]; then
  # Match the native Chromium harness's configurable watchdog.
  exec timeout --foreground "${FLECHA_HARNESS_TIMEOUT_MS:-600000}e-3s" \
    env FLECHA_SPIKE_WATCHDOG=1 bash "$0" "$@"
fi

out=$(mktemp -d "$rewrite/target/spike1-$backend-$scale-$mode.XXXXXX")
printf 'Captures and logs: %s\n' "$out"
export XDG_RUNTIME_DIR
XDG_RUNTIME_DIR=$(mktemp -d "$XDG_RUNTIME_DIR/flecha-spike.XXXXXX")
export XDG_CONFIG_HOME="$out/config" XDG_CACHE_HOME="$out/cache"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/no-session-bus"
export FLECHA_CEF_PROFILE_DIR="$out/profile"
mkdir -p "$FLECHA_CEF_PROFILE_DIR"
unset WAYLAND_DISPLAY WAYLAND_SOCKET DISPLAY

display_pid=
demo_pid=
paste_pid=
stop() {
  for pid in "$paste_pid" "$demo_pid" "$display_pid"; do
    if [[ -n $pid ]]; then
      kill "$pid" 2>/dev/null || true
      wait "$pid" 2>/dev/null || true
    fi
  done
  rmdir "$XDG_RUNTIME_DIR" 2>/dev/null || true
}
trap stop EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

case "$backend" in
  wayland)
    export WAYLAND_DISPLAY=spike1
    weston --backend=headless --renderer=vulkan --no-config --debug --fake-seat --scale="$scale" \
      --width=1280 --height=800 --socket="$WAYLAND_DISPLAY" --log="$out/display.log" &
    display_pid=$!
    while [[ ! -S $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY ]]; do kill -0 "$display_pid"; sleep 0.1; done
    ;;
  x11)
    Xvfb -displayfd 3 -screen 0 "2560x1600x24" -dpi "$((96 * scale))" -nolisten tcp \
      3>"$out/display" >"$out/display.log" 2>&1 &
    display_pid=$!
    while [[ ! -s $out/display ]]; do kill -0 "$display_pid"; sleep 0.1; done
    read -r number < "$out/display"
    export DISPLAY=":$number"
    # GPUI's documented per-process X11 override, from the pinned X11 client.
    export GPUI_X11_SCALE_FACTOR="$scale"
    ;;
  *) exit 2 ;;
esac

mkfifo "$out/control"
exec 3<>"$out/control"
bash "$spike/run-linux.sh" "${args[@]}" <&3 >"$out/demo.log" 2>&1 &
demo_pid=$!
await_log() {
  while ! rg -q "$1" "$out/demo.log"; do
    kill -0 "$demo_pid" || { cat "$out/demo.log"; return 1; }
    sleep 0.1
  done
}
capture() {
  local dir=$1
  mkdir "$dir"
  if [[ $backend == wayland ]]; then
    (cd "$dir" && weston-screenshooter)
  else
    magick import -window root "$dir/x11.png"
  fi
}
await_pixels() {
  local phase=$1 width=$2 height=$3 attempt=1 dir
  while kill -0 "$demo_pid"; do
    dir="$out/ready-$phase-$attempt"
    capture "$dir"
    if node "$spike/verify-captures.mjs" --ready "$scale" "$width" "$height" "$dir"/*.png >"$dir/check.log" 2>&1; then
      printf '%s presented expected geometry on capture %s\n' "$phase" "$attempt"
      return
    fi
    ((attempt += 1))
  done
  return 1
}
await_color() {
  local phase=$1 x=$2 y=$3 expected=$4 dir="$out/$1"
  mkdir "$dir"
  while kill -0 "$demo_pid"; do
    magick import -window root "$dir/x11.png"
    if [[ $(magick "$dir/x11.png" -format "%[hex:p{$x,$y}]" info:) == "$expected" ]]; then
      return
    fi
  done
  printf 'Missing %s pixel\n' "$phase" >&2
  return 1
}

if [[ $backend == wayland && $mode != software ]]; then
  frame_type='dma-buf'
else
  frame_type='software'
fi
await_log "composited $frame_type frame: $((800 * scale))x$((480 * scale))"
await_pixels initial 800 480
capture "$out/initial1"
capture "$out/initial2"
capture "$out/initial3"
metrics=$(node "$spike/verify-captures.mjs" "$scale" 800 480 "$out"/initial*/*.png)
printf '%s\n' "$metrics"

if [[ -n $input ]]; then
  read -r left top < <(jq -r '.origin | @tsv' <<< "$metrics")
  cc -std=c17 -Wall -Wextra -Werror "$spike/x11-input.c" -lX11 -lXtst -o "$out/x11-input"
  "$out/x11-input" click "$((left + 110))" "$((top + 103))"
  await_log 'CEF popup paint'
  capture "$out/popup"
  popup_region="60x65+$((left + 82))+$((top + 109))"
  magick "$out/initial3/x11.png" -crop "$popup_region" -depth 8 "rgb:$out/popup/before.rgb"
  magick "$out/popup/x11.png" -crop "$popup_region" -depth 8 "rgb:$out/popup/after.rgb"
  if cmp --silent "$out/popup/before.rgb" "$out/popup/after.rgb"; then
    printf 'CEF popup did not change the captured page pixels.\n' >&2
    exit 1
  fi
  printf 'CEF popup painted over the page.\n'
  "$out/x11-input" click "$((left + 105))" "$((top + 151))"
  await_log 'CEF popup hidden'
  capture "$out/popup-hidden"
  "$out/x11-input" click "$((left + 220))" "$((top + 103))"
  await_color focused "$((left + 28))" "$((top + 468))" 53C9FF
  "$out/x11-input" key x
  await_color typed "$((left + 12))" "$((top + 468))" 48D159
  printf 'The page input received the X11 key event.\n'
  if [[ $input == clipboard ]]; then
    "$out/x11-input" copy
    capture "$out/copied"
    "$out/x11-input" paste &
    paste_pid=$!
    wait "$paste_pid"
    paste_pid=
    printf 'A second X11 client received the typed x from the clipboard.\n'
  fi
fi

printf 'resize 640 360\n' >&3
await_log "composited $frame_type frame: $((640 * scale))x$((360 * scale))"
await_pixels resized 640 360
capture "$out/resized1"
capture "$out/resized2"
capture "$out/resized3"
node "$spike/verify-captures.mjs" "$scale" 640 360 "$out"/resized*/*.png

if [[ $mode == fallback ]]; then
  printf 'reject-next-import\n' >&3
  await_log 'accelerated OSR failed, creating a software browser:.*Invalid DMA-BUF'
  await_log "composited software frame: $((640 * scale))x$((360 * scale))"
  await_pixels fallback 640 360
  capture "$out/fallback1"
  capture "$out/fallback2"
  capture "$out/fallback3"
  node "$spike/verify-captures.mjs" "$scale" 640 360 "$out"/fallback*/*.png
fi

printf 'close\n' >&3
wait "$demo_pid"
demo_pid=
cat "$out/demo.log"
