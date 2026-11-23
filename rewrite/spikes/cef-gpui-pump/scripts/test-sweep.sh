#!/usr/bin/env bash
# Exercises scripts/sweep.sh, the runner's end-of-run sweep, without CEF:
#   1. a failed `ps` inventory on the first pass is an incomplete sweep (3);
#   2. a failed `ps` inventory on a retry is an incomplete sweep (3);
#   3. a failed marker sweep is an incomplete sweep (3);
#   4. stop_run, as the runner's EXIT trap calls it with a runtime dir but no
#      recorded group and no launcher left in `jobs -p`, still kills a marked
#      process that is not a child of this shell.
# The marked processes carry a runtime dir made here, so no other process
# carries the marker, and each starts its own session with `setsid -f`, so it
# is not a job of this shell. On exit the sweeper kills whatever still carries
# the marker. Exits nonzero unless every case holds.
#
# Usage: scripts/test-sweep.sh SWEEPER
#   SWEEPER  the runner's bundled app binary (its --pump-sweep modes read /proc
#            only). Takes STEP_TIMEOUT_MS from the environment, as the runner does.
set -euo pipefail

(( $# == 1 )) || { echo "usage: $0 SWEEPER" >&2; exit 2; }
sweeper=$1
[[ -x $sweeper ]] || { echo "sweeper $sweeper is not executable" >&2; exit 2; }
: "${STEP_TIMEOUT_MS:?}"
[[ $STEP_TIMEOUT_MS =~ ^[1-9][0-9]*$ ]] || { echo "STEP_TIMEOUT_MS must be a positive whole number" >&2; exit 2; }
here=$(cd "$(dirname "$0")/.." && pwd)
# shellcheck source=sweep.sh
source "$here/scripts/sweep.sh"
real_ps=$(command -v ps)

work=$(mktemp -d -p /tmp cef-gpui-pump-sweep-test.XXXXXX)
[[ $work =~ ^/tmp/cef-gpui-pump-sweep-test\.[A-Za-z0-9]+$ ]] || { echo "unexpected work dir $work" >&2; exit 1; }
runtime=$work/runtime
mkdir -m 700 "$runtime" "$work/shim"
on_exit() {
    local status=$?
    env -i "$sweeper" --pump-sweep-kill="$runtime" >/dev/null || status=1
    rm -rf -- "$work"
    exit "$status"
}
trap on_exit EXIT

# A `ps` that runs the real one for its first $PS_OK_CALLS calls and fails
# after, with no output, as a ps that cannot read /proc would.
cat >"$work/shim/ps" <<'EOF'
#!/usr/bin/env bash
n=$(( $(cat "$PS_COUNT" 2>/dev/null || echo 0) + 1 ))
echo "$n" >"$PS_COUNT"
(( n <= PS_OK_CALLS )) && exec "$REAL_PS" "$@"
echo "ps shim: failing call $n" >&2
exit 1
EOF
chmod +x "$work/shim/ps"

# Starts a marked `sleep` in a session of its own and prints its PID once it
# has exec'd sleep. Its stdio is /dev/null, so it holds no pipe of a caller's
# command substitution open.
marked_sleep() {
    local pid_file=$work/pid.$RANDOM pid=''
    setsid -f env XDG_RUNTIME_DIR="$runtime" sh -c 'echo $$ >"$1"; exec sleep 600' sh "$pid_file" \
        </dev/null >/dev/null 2>&1
    for _ in $(seq 1 $(( (STEP_TIMEOUT_MS + 9) / 10 ))); do
        [[ -s $pid_file ]] && pid=$(<"$pid_file")
        [[ -n $pid && $(cat "/proc/$pid/comm" 2>/dev/null) == sleep ]] && { echo "$pid"; return 0; }
        sleep 0.01
    done
    echo "marked sleep did not start" >&2
    return 1
}
alive() { local s; s=$(awk '{ sub(/.*\) /, ""); print $1 }' "/proc/$1/stat" 2>/dev/null) && [[ $s != Z ]]; }
fail() { echo "FAIL: $1" >&2; exit 1; }
with_ps() { # ok_calls command...
    local ok=$1
    shift
    rm -f -- "$work/ps.count"
    PATH=$work/shim:$PATH PS_COUNT=$work/ps.count PS_OK_CALLS=$ok REAL_PS=$real_ps "$@"
}

echo "== 1. ps fails on the first pass"
p=$(marked_sleep)
rc=0
out=$(with_ps 0 end_group "$sweeper" "$p" "$runtime" 2>&1) || rc=$?
printf '%s\n' "$out" | sed 's/^/   /'
(( rc == 3 )) || fail "end_group returned $rc, not 3, after a failed first inventory"
echo "   returned 3"

echo "== 2. ps fails on a retry"
rc=0
out=$(with_ps 1 end_group "$sweeper" "$p" "$runtime" 2>&1) || rc=$?
printf '%s\n' "$out" | sed 's/^/   /'
(( rc == 3 )) || fail "end_group returned $rc, not 3, after a failed retry inventory"
[[ $(cat "$work/ps.count") == 2 ]] || fail "ps ran $(cat "$work/ps.count") times, not twice"
# $p leads its own group, so the group kill or the sweeper ended it.
alive "$p" && fail "$p survived"
echo "   returned 3 after the retry; $p was killed on the first pass"

echo "== 3. the marker sweep fails"
p=$(marked_sleep)
rc=0
# A sweeper that lists like the real one and fails every kill.
cat >"$work/shim/broken-sweeper" <<EOF
#!/usr/bin/env bash
case \$1 in --pump-sweep=*) exec "$sweeper" "\$@" ;; esac
exit 1
EOF
chmod +x "$work/shim/broken-sweeper"
out=$(end_group "$work/shim/broken-sweeper" '' "$runtime" 2>&1) || rc=$?
printf '%s\n' "$out" | sed 's/^/   /'
(( rc == 3 )) || fail "end_group returned $rc, not 3, after a failed marker sweep"
echo "   returned 3"

echo "== 4. stop_run with a runtime dir and no group or launcher"
alive "$p" || fail "$p is gone before stop_run"
[[ -z $(jobs -p) ]] || fail "this shell has jobs: $(jobs -p)"
out=$(stop_run "$sweeper" '' '' "$runtime" 2>&1)
printf '%s\n' "$out" | sed 's/^/   /'
awk -v p="$p" '$1 == p' <<<"$out" | grep -c . >/dev/null || fail "stop_run did not list $p"
grep -Fxc "sweep: killed $p" <<<"$out" >/dev/null || fail "stop_run did not kill $p"
alive "$p" && fail "$p survived stop_run"
echo "   $p listed and killed by the marker sweep"
echo "all sweep cases passed"
