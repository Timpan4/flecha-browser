#!/usr/bin/env bash
set -euo pipefail

source "$(dirname -- "${BASH_SOURCE[0]}")/../spikes/gpui-cef-compositor/runtime-paths.sh"
runtime_paths linux64 cef_linux_x86_64
check_runtime
cd "$rewrite"
suite=$(mktemp -d "$rewrite/target/phase1-controls.XXXXXX")
logs="$suite/logs"
shadow="$suite/rewrite"
mkdir -p "$logs" "$shadow/target"
printf 'Negative control logs: %s\n' "$logs"
cp -a --reflink=auto Cargo.toml Cargo.lock clippy.toml .cargo app crates spikes "$shadow/"
ln -s "$sources" "$shadow/target/spike1-src"
launcher=(flock "${XDG_RUNTIME_DIR:?}/flecha-rewrite-build.lock" bash
  "$HOME/.claude/skills/hosts/scripts/agent-build" run "$HOME/.cargo/bin/cargo")

expect_failure() {
  local name=$1 expected=$2
  shift 2
  if "$@" >"$logs/$name.log" 2>&1; then
    printf 'FAIL %s unexpectedly passed\n' "$name" >&2
    return 1
  fi
  if ! grep -F -c "$expected" "$logs/$name.log" >/dev/null; then
    cat "$logs/$name.log" >&2
    printf 'FAIL %s did not fail at the expected assertion\n' "$name" >&2
    return 1
  fi
  printf 'PASS %s failed at the expected assertion\n' "$name"
}

expect_failure direct-launch 'Flecha must be started by rewrite/tools/isolated-run.sh' \
  env -i LD_LIBRARY_PATH="$runtime" "$rewrite/target/debug/flecha"
expect_failure restore 'restore lost the persisted window' tools/verify-slice.sh break-restore
expect_failure resolver 'Public name localtest.me resolved inside the launcher' \
  tools/isolated-run.sh x11 -- bash -c 'printf "127.0.0.1 localtest.me\n" >> /mnt/scratch/hosts
    exec bash tools/verify-launcher-isolation.sh'
expect_failure runtime-socket 'Host runtime sockets are visible inside the launcher' \
  tools/isolated-run.sh x11 -- node -e '
    const { spawnSync } = require("node:child_process");
    require("node:net").createServer().listen("/run/fixture.sock", () => {
      const result = spawnSync("bash", ["tools/verify-launcher-isolation.sh"], { stdio: "inherit" });
      process.exit(result.status ?? 1);
    });'

binary="./target/${suite##*/}/rewrite/target/debug/flecha"
for mutation in cookie stale stale-paint stale-title split-swap blank-restart close-archive peek-shared; do
  case "$mutation" in
    cookie|peek-shared) expected='cookie request used the wrong Space context' ;;
    stale|stale-paint|stale-title) expected='stale CEF generation reached the shell' ;;
    split-swap) expected='split put the fixture pages on the wrong split sides' ;;
    blank-restart) expected='restart-split did not paint both split pages' ;;
    close-archive) expected='closing did not add a fresh archive entry' ;;
  esac
  (
    cd "$shadow"
    cp -a "$rewrite/crates/." crates/
    node "$rewrite/tools/mutate-slice.mjs" "$mutation"
    "${launcher[@]}" build --locked -p flecha -j 14 >"$logs/$mutation-build.log" 2>&1
  ) || { cat "$logs/$mutation-build.log" >&2; exit 1; }
  expect_failure "$mutation" "$expected" tools/verify-slice.sh slice "$binary"
done

(
  cd "$shadow"
  cp -a "$rewrite/crates/." crates/
  node "$rewrite/tools/mutate-slice.mjs" timeout
  expect_failure timeout 'use of a disallowed method' \
    "${launcher[@]}" clippy --locked -p flecha -j 14 --all-targets -- -D warnings
  grep -F 'FutureExt::with_timeout' "$logs/timeout.log"
)
