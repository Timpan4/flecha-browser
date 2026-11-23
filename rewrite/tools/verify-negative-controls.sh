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
  local name=$1 expected=$2 status=0
  shift 2
  "$@" >"$logs/$name.log" 2>&1 || status=$?
  if (( status == 0 )); then
    printf 'FAIL %s unexpectedly passed\n' "$name" >&2
    return 1
  fi
  if [[ $expected == 'RETRY '* ]]; then
    local progress
    progress=$(grep -E '^(PASS|READY|FAIL|RETRY) ' "$logs/$name.log" | tail -n 1) || progress=''
    if (( status == 124 )) && [[ $progress == "$expected"* ]]; then
      printf 'PASS %s reached the watchdog with %s\n' "$name" "$progress"
      return
    fi
  elif grep -F -c "$expected" "$logs/$name.log" >/dev/null; then
    printf 'PASS %s failed at the expected assertion\n' "$name"
    return
  fi
  cat "$logs/$name.log" >&2
  printf 'FAIL %s did not fail at the expected assertion (exit %s)\n' "$name" "$status" >&2
  return 1
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
slice_mutations=(cookie stale stale-paint stale-title split-swap blank-restart close-archive
  peek-shared pane-size pane-height archive-dup stale-closedat-ui stale-closedat
  pane-offset ratio-002 ratio-001 right-both pane-size-restart peek-blind-click flechinha-old-partition)
for mutation in "${slice_mutations[@]}"; do
  case "$mutation" in
    cookie|peek-shared) expected='cookie request used the wrong Space context' ;;
    stale|stale-paint|stale-title) expected='stale CEF generation reached the shell' ;;
    split-swap|pane-size|pane-height|pane-offset|ratio-002|ratio-001) expected='RETRY split-geometry: unexpected' ;;
    pane-size-restart) expected='RETRY restart-split-geometry: unexpected' ;;
    right-both) expected='RETRY split: missing fixture colors #147A70, #FFCC49' ;;
    blank-restart) expected='RETRY restart-split: blank restart control rendered its replacement frame' ;;
    close-archive) expected='closing did not add a fresh archive entry' ;;
    archive-dup) expected='new archive entry belongs to a different page' ;;
    stale-closedat|stale-closedat-ui) expected='archive timestamp predates the close command' ;;
    peek-blind-click) expected='Peek backdrop passed the click through to Separate cookies' ;;
    flechinha-old-partition) expected='partition change left a Flechinha on the previous cookie jar' ;;
  esac
  (
    cd "$shadow"
    # Reverting a mutation must advance source mtimes for Cargo's fingerprint.
    cp -R "$rewrite/crates/." crates/
    node "$rewrite/tools/mutate-slice.mjs" "$mutation"
    "${launcher[@]}" build --locked -p flecha -j 14 >"$logs/$mutation-build.log" 2>&1
  ) || { cat "$logs/$mutation-build.log" >&2; exit 1; }
  expect_failure "$mutation" "$expected" tools/verify-slice.sh slice "$binary"
done

(
  cd "$shadow"
  cp -R "$rewrite/crates/." crates/
  node "$rewrite/tools/mutate-slice.mjs" stale-openlink
  expect_failure stale-openlink 'retired callback reached the shell' \
    "${launcher[@]}" test --locked -p flecha-cef -j 14 --lib -- \
    tests::rejects_every_callback_from_a_retired_generation --exact
)

(
  cd "$shadow"
  cp -R "$rewrite/crates/." crates/
  node "$rewrite/tools/mutate-slice.mjs" new-variant
  "${launcher[@]}" build --locked -p flecha-cef -j 14 >"$logs/new-variant-build.log" 2>&1
  expect_failure new-variant 'non-exhaustive patterns: `&Event::Favicon(_)` not covered' \
    "${launcher[@]}" test --locked -p flecha-cef -j 14 --lib --no-run
)

(
  cd "$shadow"
  cp -R "$rewrite/crates/." crates/
  node "$rewrite/tools/mutate-slice.mjs" timeout
  expect_failure timeout 'use of a disallowed method' \
    "${launcher[@]}" clippy --locked -p flecha -j 14 --all-targets -- -D warnings
  grep -F 'FutureExt::with_timeout' "$logs/timeout.log"
)

rm -rf --one-file-system -- "$shadow"
