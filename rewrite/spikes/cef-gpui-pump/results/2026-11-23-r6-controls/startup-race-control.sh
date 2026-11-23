#!/usr/bin/env bash
# Shows that run-checks.sh stops a run that is being launched when the runner
# is told to stop, before the session's process group exists. A `timeout` shim
# first on PATH records its PID and waits STEP_TIMEOUT_MS with a shell builtin
# before it execs the real timeout, which is what creates the group. While the
# shim waits, the control sends SIGTERM to the runner. Exits nonzero unless the
# runner exits, the shim is gone, and no process of the run started.
#
# If the runner leaves the shim alive, the control kills it by PID itself
# before it can exec timeout, so a failing run starts no session.
#
# Usage: startup-race-control.sh OUT_DIR [RUNNER]
#   RUNNER defaults to this checkout's scripts/run-checks.sh. Takes WARMUP_MS,
#   MEASURE_MS and STEP_TIMEOUT_MS from the environment, as run-checks.sh does.
set -euo pipefail

(( $# == 1 || $# == 2 )) || { echo "usage: $0 OUT_DIR [RUNNER]" >&2; exit 2; }
: "${WARMUP_MS:?}" "${MEASURE_MS:?}" "${STEP_TIMEOUT_MS:?}"
[[ $STEP_TIMEOUT_MS =~ ^[0-9]+$ ]] || { echo "STEP_TIMEOUT_MS must be digits" >&2; exit 2; }
here=$(cd "$(dirname "$0")/../.." && pwd)
runner_script=${2:-$here/scripts/run-checks.sh}
[[ -x $runner_script ]] || { echo "runner $runner_script is not executable" >&2; exit 2; }
real_timeout=$(command -v timeout) || { echo "no timeout on PATH" >&2; exit 2; }
mkdir -p "$1"
out=$(cd "$1" && pwd)
shim_dir=$out/shim
mkdir -p "$shim_dir"
pid_file=$out/shim.pid
rm -f -- "$pid_file"

# The shim waits with `read -t` on a pipe nobody writes, so it starts no child
# that could outlive it.
cat >"$shim_dir/timeout" <<EOF
#!/usr/bin/env bash
echo \$\$ >"$pid_file"
exec {fd}<> <(:)
read -r -t "$(( (10#$STEP_TIMEOUT_MS + 999) / 1000 ))" -u "\$fd" || true
exec "$real_timeout" "\$@"
EOF
chmod +x "$shim_dir/timeout"

on_exit() {
    local status=$? job
    for job in $(jobs -p); do
        kill -TERM "$job" 2>/dev/null || true
        wait "$job" 2>/dev/null || true
    done
    exit "$status"
}
trap on_exit EXIT

PATH=$shim_dir:$PATH "$runner_script" "$out" idle >"$out/run.log" 2>&1 &
runner=$!
# The runner's setup steps are each bounded by the runner itself, so wait for
# the shim as long as the runner runs.
while [[ ! -s $pid_file ]] && kill -0 "$runner" 2>/dev/null; do sleep 0.1; done
[[ -s $pid_file ]] || { echo "the runner exited before launching a session; see $out/run.log" >&2; exit 1; }
shim=$(<"$pid_file")
shim_pgid=$(ps -o pgid= -p "$shim" | tr -d ' ')
echo "shim $shim waiting, process group $shim_pgid (its own group would be $shim)" | tee "$out/control.log"
kill -TERM "$runner"
status=0
wait "$runner" || status=$?
echo "runner exit $status" | tee -a "$out/control.log"

shim_alive=
if [[ -r /proc/$shim/cmdline ]] && tr '\0' ' ' <"/proc/$shim/cmdline" | grep -Fc "$shim_dir/timeout" >/dev/null; then
    shim_alive=yes
    kill -KILL "$shim"
fi
echo "shim alive after the runner: ${shim_alive:-no}" | tee -a "$out/control.log"
started=$(pgrep -a -f -- "--pump-out=$out/idle.json" || true)
echo "processes of the run that started: ${started:-none}" | tee -a "$out/control.log"
[[ -z $shim_alive ]] || { echo "the runner left the launching shim alive" >&2; exit 1; }
[[ -z $started ]] || { echo "the run started after the runner stopped" >&2; exit 1; }
(( status != 0 )) || { echo "the runner exited 0 after SIGTERM" >&2; exit 1; }
echo "the runner stopped the launch before its process group existed" | tee -a "$out/control.log"
