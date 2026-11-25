#!/usr/bin/env bash
set -euo pipefail

if ! node_major=$(node -p 'process.versions.node.split(".")[0]' 2>/dev/null) || (( node_major < 24 )); then
  printf 'The CEF extensions spike requires Node.js 24 or newer.\n' >&2
  exit 1
fi
if [[ ! -x /usr/bin/bsdtar ]]; then
  printf 'The CEF extensions spike requires bsdtar at /usr/bin/bsdtar for CRX packaging in the isolated runner.\n' >&2
  exit 1
fi

here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
source "$here/../gpui-cef-compositor/runtime-paths.sh"
runtime_paths linux64 cef_linux_x86_64
check_runtime

out=$(mktemp -d "$rewrite/target/cef-osr-extensions.XXXXXX")
cp -R "$here/extension" "$here/run.mjs" "$out/"
printf '%s' 'iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAIAAACQkWg2AAAAGUlEQVQoz2NUdU9nIAUwkaR6VMOohiGlAQC4dgDzFwxe6wAAAABJRU5ErkJggg==' |
  base64 --decode > "$out/extension/icon.png"
c++ -std=c++20 -Wall -Wextra -Werror -DCEF_API_VERSION=15800 -I "$runtime" \
  "$here/probe.cc" -L "$runtime" -lcef -pthread -o "$out/probe"

ulimit -c 0
export FLECHA_VISUAL_OUT="$out"
exec bash "$rewrite/tools/isolated-run.sh" x11 --binary "$out/probe" -- \
  node /mnt/visual/run.mjs "$@"
