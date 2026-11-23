#!/usr/bin/env bash
set -euo pipefail

usage() {
  printf 'Usage: %s x11|wayland -- COMMAND [ARGS...]\n' "${0##*/}" >&2
  exit 2
}

if [[ ${1:-} != --inside ]]; then
  [[ $# -ge 3 && $2 == -- ]] || usage
  case "$1" in x11|wayland) ;; *) usage ;; esac
  source "$(dirname -- "${BASH_SOURCE[0]}")/../spikes/gpui-cef-compositor/runtime-paths.sh"
  runtime_paths linux64 cef_linux_x86_64
  check_runtime
  scratch=$(mktemp -d "$rewrite/target/phase1.XXXXXX")
  printf 'Isolated run: %s\n' "$scratch"
  backend=$1
  shift 2
  node_mount=()
  if node_path=$(command -v node); then
    node_mount=(--ro-bind "$(realpath "$node_path")" /mnt/bin/node)
  fi

  # The shipping Chromium native harness supplies the default watchdog.
  # A private PID namespace reaps CEF helpers, the fixture, the display and the bus.
  status=0
  timeout --foreground "${FLECHA_HARNESS_TIMEOUT_MS:-600000}e-3s" \
    bwrap --unshare-pid --unshare-ipc --unshare-net --new-session --die-with-parent \
    --cap-add CAP_NET_ADMIN --cap-add CAP_SETPCAP \
    --ro-bind / / --dev-bind /dev /dev --tmpfs /dev/shm --proc /proc \
    --tmpfs /run/user --tmpfs /run/dbus --tmpfs /tmp --tmpfs "$HOME" --tmpfs /mnt \
    --ro-bind "$rewrite" /mnt/work --bind "$scratch" /mnt/scratch --chdir /mnt/work \
    "${node_mount[@]}" --clearenv --setenv PATH /mnt/bin:/usr/bin:/bin --setenv LANG "${LANG:-C}" \
    --setenv FLECHA_CEF_RUNTIME_DIR "/mnt/work/${runtime#"$rewrite/"}" \
    --setenv FLECHA_CEF_RUNTIME_SHA1 "$sha1" \
    bash /mnt/work/tools/isolated-run.sh --inside "$backend" "$@" || status=$?
  printf 'Isolated process namespace exited (%s)\n' "$status"
  exit "$status"
fi

shift
backend=$1
shift
ip link set lo up
exec setpriv --bounding-set=-all --inh-caps=-all --ambient-caps=-all \
  bash /mnt/work/tools/isolated-session.sh "$backend" "$@"
