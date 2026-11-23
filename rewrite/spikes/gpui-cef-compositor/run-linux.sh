#!/usr/bin/env bash
set -euo pipefail

source "$(dirname -- "${BASH_SOURCE[0]}")/runtime-paths.sh"
runtime_paths linux64 cef_linux_x86_64
check_runtime
if [[ ${DBUS_SESSION_BUS_ADDRESS:-} != "unix:path=${XDG_RUNTIME_DIR:-}/bus" ||
  ${DBUS_SYSTEM_BUS_ADDRESS:-} != "$DBUS_SESSION_BUS_ADDRESS" ||
  ! -S ${XDG_RUNTIME_DIR:-}/bus ]]; then
  printf 'Run the demo through verify-linux.sh on an isolated display and private D-Bus.\n' >&2
  exit 1
fi
export FLECHA_CEF_RUNTIME_DIR="$runtime"
export FLECHA_CEF_RUNTIME_SHA1="$sha1"
export FLECHA_CEF_PROFILE_DIR="${FLECHA_CEF_PROFILE_DIR:-$(mktemp -d "$rewrite/target/cef-profile.XXXXXX")}"
export LD_LIBRARY_PATH="$FLECHA_CEF_RUNTIME_DIR${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
exec "$rewrite/target/debug/gpui-cef-compositor" "$@"
