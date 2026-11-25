#!/usr/bin/env bash
set -euo pipefail

usage() {
  printf 'Usage: %s x11|wayland [--network] [--binary PATH] -- COMMAND [ARGS...]\n' "${0##*/}" >&2
  exit 2
}

[[ $# -ge 3 ]] || usage
case "$1" in x11|wayland) ;; *) usage ;; esac
source "$(dirname -- "${BASH_SOURCE[0]}")/../spikes/gpui-cef-compositor/runtime-paths.sh"
runtime_paths linux64 cef_linux_x86_64
check_runtime
backend=$1
shift
network=offline
if [[ $1 == --network ]]; then
  command -v pasta >/dev/null || { printf 'Networked isolation requires pasta\n' >&2; exit 1; }
  network=online
  shift
fi
binary="$rewrite/target/debug/flecha"
if [[ $1 == --binary ]]; then
  [[ $# -ge 4 ]] || usage
  binary=$2
  if [[ $binary != /* ]]; then binary="$rewrite/$binary"; fi
  [[ -f $binary ]] || { printf 'App binary is missing\n' >&2; exit 1; }
  shift 2
fi
[[ $# -ge 2 && $1 == -- ]] || usage
shift

scratch=$(mktemp -d "$rewrite/target/phase1.XXXXXX")
printf 'Isolated run: %s\n' "$scratch"
printf '127.0.0.1 localhost\n::1 localhost\n' > "$scratch/hosts"
network_command=()
network_namespace=(--unshare-net)
network_mounts=()
if [[ $network == online ]]; then
  # Match Podman's pasta DNS forwarding address. The namespace has no host port forwarding.
  dns=169.254.1.1
  network_command=(pasta --config-net --no-map-gw -t none -u none -T none -U none --dns-forward "$dns" --)
  # pasta maps the invoking user to root. Restore the non-root UID used by CEF and D-Bus.
  network_namespace=(--unshare-user --uid "$(id -u)" --gid "$(id -g)")
  network_mounts=(--ro-bind-try /etc/ssl/certs /etc/ssl/certs --ro-bind-try /etc/ca-certificates /etc/ca-certificates)
  printf 'hosts: files dns\npasswd: files\ngroup: files\n' > "$scratch/nsswitch.conf"
  printf 'nameserver %s\n' "$dns" > "$scratch/resolv.conf"
else
  printf 'hosts: files\npasswd: files\ngroup: files\n' > "$scratch/nsswitch.conf"
  printf '# No DNS servers in the isolated launcher.\n' > "$scratch/resolv.conf"
fi
printf 'flecha:x:%s:%s:Flecha:/mnt/scratch/home:/bin/bash\n' "$(id -u)" "$(id -g)" > "$scratch/passwd"
printf 'flecha:x:%s:\n' "$(id -g)" > "$scratch/group"

system_aliases=()
if [[ -d /usr/local ]]; then system_aliases+=(--tmpfs /usr/local); fi
for directory in /bin /sbin /lib /lib64; do
  if [[ -L $directory ]]; then
    system_aliases+=(--symlink "$(readlink "$directory")" "$directory")
  elif [[ -d $directory ]]; then
    system_aliases+=(--ro-bind "$directory" "$directory")
  fi
done
node_mount=()
if node_path=$(command -v node); then
  node_mount=(--ro-bind "$(realpath "$node_path")" /mnt/bin/node)
fi
devices=()
for device in null zero full random urandom tty; do
  devices+=(--dev-bind "/dev/$device" "/dev/$device")
done
if [[ $backend == wayland ]]; then
  # The NVIDIA driver's Vulkan ICD needs its control nodes besides /dev/dri.
  for device in nvidiactl nvidia0 nvidia-modeset; do
    devices+=(--dev-bind-try "/dev/$device" "/dev/$device")
  done
fi

# The visual mode keeps its captures: the scratch directory is pruned when the run passes.
visual=()
if [[ -n ${FLECHA_VISUAL_OUT:-} ]]; then
  mkdir -p -- "$FLECHA_VISUAL_OUT"
  visual=(--bind "$FLECHA_VISUAL_OUT" /mnt/visual)
fi

cc -std=c17 -Wall -Wextra -Werror "$rewrite/tools/pidfd-run.c" -o "$scratch/pidfd-run"
# The shipping Chromium native harness supplies the watchdog. The command
# runs as PID 1: its exit makes the kernel reap the private namespace.
exec "$scratch/pidfd-run" "${FLECHA_HARNESS_TIMEOUT_MS:-600000}" "$scratch" \
  "${network_command[@]}" bwrap --unshare-pid --as-pid-1 --unshare-ipc "${network_namespace[@]}" --new-session --die-with-parent \
  --cap-add CAP_NET_ADMIN --cap-add CAP_SETPCAP \
  --ro-bind /usr /usr "${system_aliases[@]}" --proc /proc --ro-bind /sys /sys \
  --tmpfs /dev "${devices[@]}" --dev-bind-try /dev/dri /dev/dri --tmpfs /dev/shm \
  --symlink /proc/self/fd /dev/fd \
  --tmpfs /run --tmpfs /var/run --tmpfs /tmp --tmpfs /etc \
  --ro-bind "$scratch/nsswitch.conf" /etc/nsswitch.conf \
  --ro-bind "$scratch/hosts" /etc/hosts --ro-bind "$scratch/resolv.conf" /etc/resolv.conf \
  --ro-bind "$scratch/passwd" /etc/passwd --ro-bind "$scratch/group" /etc/group \
  --ro-bind-try /etc/fonts /etc/fonts --ro-bind-try /etc/ld.so.cache /etc/ld.so.cache \
  "${network_mounts[@]}" \
  --ro-bind "$rewrite/tools" /mnt/work/tools \
  --ro-bind "$rewrite/spikes/gpui-cef-compositor/x11-input.c" /mnt/work/spikes/gpui-cef-compositor/x11-input.c \
  --ro-bind-try "$binary" /mnt/work/target/debug/flecha \
  --ro-bind "$runtime" /mnt/cef --bind "$scratch" /mnt/scratch --chdir /mnt/work \
  "${visual[@]}" \
  "${node_mount[@]}" --clearenv --setenv PATH /mnt/bin:/usr/bin:/bin \
  bash -c 'if [[ $(readlink /proc/self/ns/net) == "$1" ]]; then
      printf "Harness inherited the host network namespace\n" >&2; exit 1
    fi
    shift
    if [[ $4 == offline ]]; then ip link set lo up || exit; fi
    exec setpriv --bounding-set=-all --inh-caps=-all --ambient-caps=-all \
    bash /mnt/work/tools/isolated-session.sh "$@"' launcher \
  "$(readlink /proc/self/ns/net)" "$backend" /mnt/cef "$sha1" "$network" "$@"
