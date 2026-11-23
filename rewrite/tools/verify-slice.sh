#!/usr/bin/env bash
set -euo pipefail

if [[ ${FLECHA_LAUNCHER:-} != isolated ]]; then
  exec "$(dirname -- "${BASH_SOURCE[0]}")/isolated-run.sh" x11 -- \
    bash tools/verify-slice.sh "$@"
fi
[[ $# == 0 ]] || exit 2

out=$HOME/..
node tools/fixture.mjs "$out/fixture.port" &
fixture=$!
until [[ -s $out/fixture.port ]]; do kill -0 "$fixture"; sleep 0.1; done
read -r port < "$out/fixture.port"
base="http://127.0.0.1:$port"

mkfifo "$out/control"
exec 3<>"$out/control"
./target/debug/flecha --verify --open "$base/open" <&3 3<&- >"$out/app.log" 2>&1 &
app=$!

await_log() {
  until grep -c -- "$1" "$out/app.log" >/dev/null; do
    if ! kill -0 "$app"; then cat "$out/app.log"; return 1; fi
    sleep 0.1
  done
}

await_log '^cef load 200$'
await_log '^cef paint$'
while true; do
  kill -0 "$app"
  magick import -window root "$out/open.png"
  magick "$out/open.png" -format %c histogram:info: >"$out/histogram"
  if grep -c '#147A70' "$out/histogram" >/dev/null && grep -c '#FFCC49' "$out/histogram" >/dev/null; then
    break
  fi
  sleep 0.1
done
printf 'GPUI capture contains both fixture colors\n'
printf 'quit\n' >&3
wait "$app"
await_log '^cef closed$'
kill "$fixture"
wait "$fixture" || [[ $? == 143 ]]
printf 'CEF opened, loaded, painted and closed the fixture\n'
