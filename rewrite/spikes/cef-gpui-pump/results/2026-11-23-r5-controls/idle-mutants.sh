#!/usr/bin/env bash
# Applies each idle-rule mutant to src/idle.rs in a clean checkout, runs the
# CEF-free tests, prints the failing tests, and restores the file with git.
# Usage: idle-mutants.sh REWRITE_DIR
set -uo pipefail
rewrite=$1
file=$rewrite/spikes/cef-gpui-pump/src/idle.rs
[[ -z $(git -C "$rewrite" status --porcelain -- "$file") ]] || { echo "idle.rs has local edits" >&2; exit 1; }

mutant() {
    local name=$1 old=$2 new=$3
    echo "=== $name"
    OLD=$old NEW=$new perl -0pi -e '$c = s/\Q$ENV{OLD}\E/$ENV{NEW}/; die "pattern not found\n" unless $c' "$file" || return
    (cd "$rewrite" && flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
        bash ~/.claude/skills/hosts/scripts/agent-build run cargo test --locked --release -j 14 -p cef-gpui-pump idle:: 2>&1) |
        grep -E '^test idle::.* FAILED$|^test result'
    git -C "$rewrite" checkout -q -- "$file"
}

mutant "accept two paint-holding repaints" \
    'candidate && candidates <= 1' 'candidate && candidates <= 2'
mutant "paint-holding timeout 0 s" \
    'Duration::from_secs(4)' 'Duration::from_secs(0)'
mutant "allowance counted only inside the window (the old rule)" \
    'let candidate = p.full_view && p.after_navigation >= PAINT_HOLDING_TIMEOUT;' \
    'let candidate = p.in_window && p.full_view && p.after_navigation >= PAINT_HOLDING_TIMEOUT;'
echo "=== unmutated"
(cd "$rewrite" && flock "$XDG_RUNTIME_DIR/flecha-rewrite-build.lock" \
    bash ~/.claude/skills/hosts/scripts/agent-build run cargo test --locked --release -j 14 -p cef-gpui-pump idle:: 2>&1) |
    grep -E '^test idle::.* FAILED$|^test result'
