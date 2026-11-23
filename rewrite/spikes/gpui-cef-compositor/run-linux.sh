#!/usr/bin/env bash
set -euo pipefail

spike=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
rewrite=$(cd "$spike/../.." && pwd)
shopt -s nullglob
runtimes=("$rewrite/target/spike1-src/cef-runtime/"*/cef_linux_x86_64)
if [[ ${#runtimes[@]} != 1 || ! -f ${runtimes[0]}/libcef.so ]]; then
  printf 'Run prepare.sh for the pinned Linux CEF runtime first.\n' >&2
  exit 1
fi
export FLECHA_CEF_RUNTIME_DIR="${runtimes[0]}"
export FLECHA_CEF_PROFILE_DIR="${FLECHA_CEF_PROFILE_DIR:-$(mktemp -d "$rewrite/target/cef-profile.XXXXXX")}"
export LD_LIBRARY_PATH="$FLECHA_CEF_RUNTIME_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$rewrite/target/debug/gpui-cef-compositor" "$@"
