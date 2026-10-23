#!/bin/bash
# Builds Flecha's native messaging host (chromium/native-host) and registers it with Chromium
# for the shell extension, so it can import from other browsers and run the agent bridge.
#
#   chromium/scripts/install-native-host.sh                 # Flecha's own profile folder
#   chromium/scripts/install-native-host.sh <user-data-dir> # a --user-data-dir profile too
#   FLECHA_PROFILE=<dir> chromium/scripts/install-native-host.sh  # same, as run.sh takes it
#   chromium/scripts/install-native-host.sh --only <dir>    # that profile alone
#
# Chromium looks for user-level hosts in <user data dir>/NativeMessagingHosts
# (chrome::DIR_USER_NATIVE_MESSAGING in chrome/common/chrome_paths.cc), and the user data dir
# is ~/Library/Application Support/Flecha Browser unless --user-data-dir says otherwise
# (patch 0001). Each of those folders gets the host's manifest.
#
# The host itself (a launcher and the bundled script) is copied to $FLECHA_HOST_DIR,
# ~/Library/Application Support/Flecha Browser/native-host by default: Chromium starts hosts
# disclaiming responsibility for them (launch_context_posix.cc), so macOS privacy rules
# treat the host as its own app, which may not read ~/Documents, external disks and the
# like. Run this again after changing the host.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
name="dev.oprimo.flecha.host"
origin="chrome-extension://abdiaepfgbbhdjaecjipmaailijaaide/"
host_dir="${FLECHA_HOST_DIR:-$HOME/Library/Application Support/Flecha Browser/native-host}"

# Chromium starts the host without the shell's PATH, so the launcher names Node outright.
node="${FLECHA_NODE:-$(command -v node || true)}"
if [ -z "$node" ]; then
  echo "error: no node on PATH; set FLECHA_NODE to a Node 22.5+ binary" >&2
  exit 1
fi
if ! "$node" -e 'require("node:sqlite")' 2>/dev/null; then
  echo "error: $node has no node:sqlite; the host needs Node 22.5 or newer" >&2
  exit 1
fi

targets=()
default=1
for arg in "$@"; do
  # --only: just the folders given, not Flecha's own profile folder.
  if [ "$arg" = "--only" ]; then default=0; else targets+=("$arg"); fi
done
[ -n "${FLECHA_PROFILE:-}" ] && targets+=("$FLECHA_PROFILE")
[ "$default" = 1 ] && targets+=("$HOME/Library/Application Support/Flecha Browser")
if [ "${#targets[@]}" = 0 ]; then
  echo "error: --only needs a profile folder" >&2
  exit 1
fi

(cd "$here/.." && "$node" chromium/native-host/build.mjs)
mkdir -p "$host_dir"
cp "$here/native-host/dist/flecha-host.cjs" "$host_dir/flecha-host.cjs"
launcher="$host_dir/flecha-host"
cat >"$launcher" <<EOF
#!/bin/sh
exec "$node" "$host_dir/flecha-host.cjs" "\$@"
EOF
chmod +x "$launcher"
echo "host in $host_dir"

for dir in "${targets[@]}"; do
  hosts="$dir/NativeMessagingHosts"
  mkdir -p "$hosts"
  cat >"$hosts/$name.json" <<EOF
{
  "name": "$name",
  "description": "Flecha: imports from other browsers and the agent bridge",
  "path": "$launcher",
  "type": "stdio",
  "allowed_origins": ["$origin"]
}
EOF
  echo "installed $hosts/$name.json"
done
