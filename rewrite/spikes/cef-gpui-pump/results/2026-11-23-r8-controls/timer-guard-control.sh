#!/usr/bin/env bash
# Shows that clippy.toml rejects every GPUI API it lists, and only those. In a
# disposable copy of the committed tree (`git archive HEAD`), never the caller's
# checkout, it runs workspace Clippy with `-D warnings`, as CI does:
#   1. on the unmodified tree, which must pass with no unresolved-path warning;
#   2. with `with-timeout.patch` (the verifier's mutant C, a per-call
#      `gpui::FutureExt::with_timeout` in `Bridge::call`) and `timer_mutants.rs`
#      (one use of each listed API) added, which must fail with one
#      "use of a disallowed method/field" diagnostic for each listed path: the
#      with_timeout one at the mutant's own line and column in bridge.rs, every
#      one also in timer_mutants.rs.
# The copy and its target dir are deleted on exit.
#
# Usage: timer-guard-control.sh REWRITE_DIR
#   REWRITE_DIR  the rewrite/ dir of a checkout whose target/spike1-src was
#                staged by spikes/gpui-cef-compositor/prepare.sh
set -euo pipefail

(( $# == 1 )) || { echo "usage: $0 REWRITE_DIR" >&2; exit 2; }
[[ -d $1 ]] || { echo "not a directory: $1" >&2; exit 2; }
rewrite=$(cd "$1" && pwd)
here=$(cd "$(dirname "$0")" && pwd)
[[ -d $rewrite/target/spike1-src ]] || { echo "$rewrite/target/spike1-src is missing; run prepare.sh first" >&2; exit 2; }
: "${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set for the build lock}"
lock=$XDG_RUNTIME_DIR/flecha-rewrite-build.lock
build=$HOME/.claude/skills/hosts/scripts/agent-build
[[ -r $build ]] || { echo "build wrapper $build is not readable" >&2; exit 2; }
repo=$(git -C "$rewrite" rev-parse --show-toplevel)
head=$(git -C "$repo" rev-parse HEAD)
spike=spikes/cef-gpui-pump

copy=$(mktemp -d -p /tmp cef-gpui-pump-timer-guard.XXXXXX)
[[ $copy =~ ^/tmp/cef-gpui-pump-timer-guard\.[A-Za-z0-9]+$ ]] || { echo "unexpected copy dir $copy" >&2; exit 1; }
trap 'rm -rf -- "$copy"' EXIT
git -C "$repo" archive --format=tar "$head" rewrite | tar -x -C "$copy"
mkdir -p "$copy/rewrite/target"
ln -s "$rewrite/target/spike1-src" "$copy/rewrite/target/spike1-src"
echo "copy of $head in $copy"

clippy() {
    (cd "$copy/rewrite" && CARGO_NET_GIT_FETCH_WITH_CLI=true flock "$lock" \
        bash "$build" run cargo clippy --locked -j 14 --workspace --all-targets -- -D warnings 2>&1)
}

echo "== 1. unmodified tree"
status=0
out=$(clippy) || status=$?
grep -E '^(warning|error)' <<<"$out" || true
(( status == 0 )) || { echo "$out" >&2; echo "Clippy failed on the unmodified tree (exit $status)" >&2; exit 1; }
! grep -Fc 'reachable' <<<"$out" >/dev/null || { echo "a clippy.toml path does not resolve" >&2; exit 1; }
echo "   passed, no unresolved path"

echo "== 2. mutants"
(cd "$copy" && patch -p1 --forward --quiet <"$here/with-timeout.patch")
cp "$here/timer_mutants.rs" "$copy/rewrite/$spike/src/timer_mutants.rs"
printf '\n#[cfg(target_os = "linux")]\nmod timer_mutants;\n' >>"$copy/rewrite/$spike/src/main.rs"
at=$(awk '(c = index($0, "gpui::FutureExt::with_timeout(")) { print NR ":" c; exit }' "$copy/rewrite/$spike/src/bridge.rs")
[[ -n $at ]] || { echo "no mutant with_timeout call in bridge.rs" >&2; exit 1; }
status=0
out=$(clippy) || status=$?
(( status != 0 )) || { echo "Clippy passed the mutants" >&2; exit 1; }
# One "name file:line:col" row per disallowed-method or -field diagnostic: the
# header names the path, and rustc prints the primary span on the next line.
found=$(awk '
    match($0, /^error: use of a disallowed (method|field) `[^`]+`/) {
        name = substr($0, RSTART, RLENGTH); sub(/^[^`]*`/, "", name); sub(/`$/, "", name); next_span = 1; next
    }
    next_span { line = $0; sub(/^[ \t]*--> /, "", line); print name, line; next_span = 0 }' <<<"$out")
printf '%s\n' "$found" | sed 's/^/   /'
missing=
expected=$(sed -n 's/^ *{ path = "\([^"]*\)".*/\1/p' "$copy/rewrite/$spike/clippy.toml")
[[ -n $expected ]] || { echo "no paths in clippy.toml" >&2; exit 1; }
while read -r name; do
    awk -v n="$name" -v f="$spike/src/timer_mutants.rs" '$1 == n && index($2, f ":") == 1 { ok = 1 } END { exit !ok }' <<<"$found" ||
        missing+="$name "
done <<<"$expected"
[[ -z $missing ]] || { echo "$out" >&2; echo "no diagnostic in timer_mutants.rs for: $missing" >&2; exit 1; }
awk -v s="$spike/src/bridge.rs:$at" '$1 == "gpui::FutureExt::with_timeout" && $2 == s { ok = 1 } END { exit !ok }' <<<"$found" ||
    { echo "$out" >&2; echo "mutant C at $spike/src/bridge.rs:$at was not rejected" >&2; exit 1; }
echo "   rejected: mutant C at $spike/src/bridge.rs:$at, and each of the $(wc -l <<<"$expected") listed paths in timer_mutants.rs (Clippy exit $status)"
