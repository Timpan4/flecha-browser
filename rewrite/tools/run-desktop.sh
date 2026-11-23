#!/usr/bin/env bash
set -euo pipefail

build=debug
print_command=false
while (( $# )); do
  case "$1" in
    --release) build=release ;;
    --print-command|--dry-run) print_command=true ;;
    --) shift; break ;;
    *) break ;;
  esac
  shift
done

# shellcheck source-path=SCRIPTDIR
# shellcheck source=../spikes/gpui-cef-compositor/runtime-paths.sh
source "$(dirname -- "${BASH_SOURCE[0]}")/../spikes/gpui-cef-compositor/runtime-paths.sh"
runtime_paths linux64 cef_linux_x86_64
check_runtime

binary="$rewrite/target/$build/flecha"
if [[ ! -f $binary || ! -x $binary ]]; then
  printf 'Flecha binary is missing or not executable: %s\n' "$binary" >&2
  exit 1
fi

export FLECHA_CEF_PATH="$runtime"
export FLECHA_CEF_RUNTIME_DIR="$runtime" FLECHA_CEF_RUNTIME_SHA1="$sha1"
export LD_LIBRARY_PATH="$runtime${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
if [[ $print_command == true ]]; then
  printf '%q ' env "LD_LIBRARY_PATH=$LD_LIBRARY_PATH" "FLECHA_CEF_PATH=$FLECHA_CEF_PATH" \
    "FLECHA_CEF_RUNTIME_DIR=$FLECHA_CEF_RUNTIME_DIR" "FLECHA_CEF_RUNTIME_SHA1=$FLECHA_CEF_RUNTIME_SHA1" \
    "$binary" "$@"
  printf '\n'
  exit 0
fi
exec "$binary" "$@"
