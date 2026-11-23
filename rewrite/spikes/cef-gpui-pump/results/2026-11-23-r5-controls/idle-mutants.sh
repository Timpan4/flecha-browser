#!/usr/bin/env bash
# Applies each idle-rule mutant to src/idle.rs in a disposable copy of the
# committed tree (`git archive HEAD`), never in the caller's checkout, and runs
# the CEF-free idle tests there under the build lock. Exits nonzero unless the
# unmutated rule passes its idle tests and every mutant fails the tests named
# for it. The copy is deleted on exit.
#
# Usage: idle-mutants.sh REWRITE_DIR
#   REWRITE_DIR  the rewrite/ dir of a checkout whose target/spike1-src was
#                staged by spikes/gpui-cef-compositor/prepare.sh
set -euo pipefail

usage() { echo "usage: $0 REWRITE_DIR" >&2; exit 2; }
(( $# == 1 )) || usage
[[ -d $1 ]] || { echo "not a directory: $1" >&2; usage; }
rewrite=$(cd "$1" && pwd)
[[ -f $rewrite/Cargo.toml && -f $rewrite/spikes/cef-gpui-pump/src/idle.rs ]] ||
    { echo "$rewrite is not the rewrite/ dir" >&2; exit 2; }
[[ -d $rewrite/target/spike1-src ]] ||
    { echo "$rewrite/target/spike1-src is missing; run prepare.sh first" >&2; exit 2; }
: "${XDG_RUNTIME_DIR:?XDG_RUNTIME_DIR must be set for the build lock}"
[[ -d $XDG_RUNTIME_DIR ]] || { echo "XDG_RUNTIME_DIR $XDG_RUNTIME_DIR is not a directory" >&2; exit 2; }
lock=$XDG_RUNTIME_DIR/flecha-rewrite-build.lock
build=$HOME/.claude/skills/hosts/scripts/agent-build
[[ -r $build ]] || { echo "build wrapper $build is not readable" >&2; exit 2; }
repo=$(git -C "$rewrite" rev-parse --show-toplevel)
head=$(git -C "$repo" rev-parse HEAD)

copy=$(mktemp -d -p /tmp cef-gpui-pump-idle-mutants.XXXXXX)
trap 'rm -rf -- "$copy"' EXIT
git -C "$repo" archive --format=tar "$head" rewrite | tar -x -C "$copy"
mkdir -p "$copy/rewrite/target"
ln -s "$rewrite/target/spike1-src" "$copy/rewrite/target/spike1-src"
file=$copy/rewrite/spikes/cef-gpui-pump/src/idle.rs
pristine=$(<"$file")
echo "mutating a copy of $head in $copy"

# Runs the idle tests in the copy. Prints cargo's output; returns cargo's status.
idle_tests() {
    (cd "$copy/rewrite" && CARGO_NET_GIT_FETCH_WITH_CLI=true flock "$lock" \
        bash "$build" run cargo test --locked --release -j 14 -p cef-gpui-pump --lib idle:: 2>&1)
}

echo "=== unmutated"
out=$(idle_tests) || { echo "$out" >&2; echo "the unmutated idle tests failed" >&2; exit 1; }
summary=$(grep -E '^test result: ok\. [1-9][0-9]* passed' <<<"$out") ||
    { echo "$out" >&2; echo "the unmutated run passed no idle tests" >&2; exit 1; }
echo "$summary"

mutant() { # name old new expected-failing-test...
    local name=$1 old=$2 new=$3 status=0 t
    shift 3
    echo "=== $name"
    OLD=$old NEW=$new perl -0pi -e '$c = s/\Q$ENV{OLD}\E/$ENV{NEW}/; die "pattern not found\n" unless $c' "$file"
    out=$(idle_tests) || status=$?
    printf '%s\n' "$pristine" >"$file"
    grep -E '^test idle::.* FAILED$|^test result' <<<"$out" || true
    (( status != 0 )) || { echo "mutant survived: $name" >&2; exit 1; }
    grep -c -E '^test result: FAILED' <<<"$out" >/dev/null ||
        { echo "$out" >&2; echo "cargo failed without a test result (build error?): $name" >&2; exit 1; }
    for t in "$@"; do
        grep -c -F -x "test idle::tests::$t ... FAILED" <<<"$out" >/dev/null ||
            { echo "expected idle::tests::$t to fail for: $name" >&2; exit 1; }
    done
}

mutant "accept two paint-holding repaints" \
    'candidate && candidates <= 1' 'candidate && candidates <= 2' \
    a_second_repaint_in_the_window_fails a_repaint_after_an_earlier_paint_holding_repaint_fails
mutant "paint-holding timeout 0 s" \
    'Duration::from_secs(4)' 'Duration::from_secs(0)' \
    the_paint_holding_repaint_in_the_window_passes
mutant "allowance counted inside the window only (round 2's rule)" \
    'let candidate = i > 0' 'let candidate = p.in_window && i > 0' \
    a_repaint_after_an_earlier_paint_holding_repaint_fails
mutant "no upper bound on the repaint time" \
    '&& latest.is_some_and(|latest| p.after_navigation <= latest)' '' \
    a_lone_late_repaint_fails
mutant "no earlier paint required" \
    'let candidate = i > 0' 'let candidate = true' \
    a_repaint_that_is_the_first_paint_fails
echo "every mutant failed its named tests; the unmutated rule passed"
