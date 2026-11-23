#!/usr/bin/env bash
# Runs every Spike 2 check on a private Xvfb display, a private D-Bus session
# with no service activation, a scratch XDG_RUNTIME_DIR, HOME and TMPDIR per
# run, and a system bus address that points nowhere. Each app run checks that
# its process tree holds nothing under the desktop session's runtime dir, the
# system bus socket or the caller's home outside the bundle, pages and results
# (`isolation_clean` in the report). Stops what it starts by PID, by the
# process group ID of a run's session, and through pidfds for processes
# marked with the run's XDG_RUNTIME_DIR (scripts/sweep.sh): when the run
# outlives its watchdog (see `watchdog_ms`), when anything of the session
# outlives it, and when the runner itself exits mid-run.
# Exits nonzero if any run fails.
#
# Usage: scripts/run-checks.sh RESULTS_DIR [SCENARIO...]
#   SCENARIO: idle animate animate-unthrottled cdp devtools (default: all)
#
# Timing knobs. These are test-harness parameters, not product values; REPORT.md
# quotes the values each run used.
#   WARMUP_MS          time after load before measuring
#   MEASURE_MS         length of each measured window
#   STEP_TIMEOUT_MS    bound on each wait for CEF (load, a DevTools call, close)
#   DEVTOOLS_CLOSE_MS  space-separated CloseDevTools delays after ShowDevTools
#   DEVTOOLS_TRIALS    fresh processes per delay
#   EXTRA_ARGS         extra arguments, e.g. --pump-no-sandbox
set -euo pipefail
shopt -s inherit_errexit

results=${1:?usage: run-checks.sh RESULTS_DIR [SCENARIO...]}
shift
scenarios=("$@")
(( ${#scenarios[@]} )) || scenarios=(idle animate animate-unthrottled cdp devtools)

: "${WARMUP_MS:?set WARMUP_MS}" "${MEASURE_MS:?set MEASURE_MS}" "${STEP_TIMEOUT_MS:?set STEP_TIMEOUT_MS}"
# Bash arithmetic reads 0100 as octal and works in signed 64-bit integers with
# no overflow check (bash(1), ARITHMETIC EVALUATION), while the app parses
# decimal u64 milliseconds. So take digits only, refuse values past Bash's
# largest integer, and use their decimal value: the watchdog then budgets what
# the app runs.
int_max=9223372036854775807
decimal_ms() { # name value
    local v
    [[ $2 =~ ^[0-9]+$ ]] || { echo "$1 must be a whole number of milliseconds, got '$2'" >&2; exit 2; }
    v=${2#"${2%%[!0]*}"}
    v=${v:-0}
    # Same length: compare the first 18 digits, which fit, then the last.
    if (( ${#v} > ${#int_max} )) || { (( ${#v} == ${#int_max} )) &&
        (( 10#${v:0:18} > 10#${int_max:0:18} ||
            (10#${v:0:18} == 10#${int_max:0:18} && ${v:18} > ${int_max:18}) )); }; then
        echo "$1=$2 is past Bash's largest integer, $int_max" >&2
        exit 2
    fi
    echo "$v"
}
WARMUP_MS=$(decimal_ms WARMUP_MS "$WARMUP_MS")
MEASURE_MS=$(decimal_ms MEASURE_MS "$MEASURE_MS")
STEP_TIMEOUT_MS=$(decimal_ms STEP_TIMEOUT_MS "$STEP_TIMEOUT_MS")
# The app refuses a zero window or step timeout (src/config.rs); so does the
# runner, whose bounded waits and timeout --kill-after need a nonzero step.
(( MEASURE_MS > 0 && STEP_TIMEOUT_MS > 0 )) || { echo "MEASURE_MS and STEP_TIMEOUT_MS must be greater than 0" >&2; exit 2; }
# The app keeps the last value of a repeated flag, so a timing flag in
# EXTRA_ARGS would run with durations the watchdog did not budget.
case " ${EXTRA_ARGS:-} " in
    *--pump-warmup-ms*|*--pump-measure-ms*|*--pump-step-timeout-ms*|*--pump-devtools-close-ms*)
        echo "set timings through WARMUP_MS, MEASURE_MS, STEP_TIMEOUT_MS and DEVTOOLS_CLOSE_MS, not EXTRA_ARGS" >&2
        exit 2 ;;
esac

# The longest a run may take, from its own knobs: its measured windows (each a
# warmup plus a measurement) plus one STEP_TIMEOUT_MS for every wait the app
# bounds and every synchronous CEF call it cannot bound (CefInitialize,
# CreateBrowserSync per page, ShowDevTools, CefShutdown). Counted from
# src/scenario.rs and src/app.rs:
#   every run: CefInitialize, context ready, CreateBrowserSync per page, page
#     load, every OnBeforeClose, CefShutdown (5 + pages)
#   idle: 1 window
#   animate: 2 windows, 4 page counter reads
#   cdp: 1 window, list tabs, 2 page counter reads, the loop's overrun bound
#     and the timeout probe running beside it (2), the screenshot
#   devtools (2 pages): 2 windows, 4 evaluates, ShowDevTools, its creation,
#     its close, plus the close delay
# a + b and k * a for non-negative operands, refusing any result past int_max.
add() { (( $2 <= int_max - $1 )) || { echo "watchdog budget past $int_max ms" >&2; exit 2; }; echo $(($1 + $2)); }
mul() { (( $1 == 0 || $2 <= int_max / $1 )) || { echo "watchdog budget past $int_max ms" >&2; exit 2; }; echo $(($1 * $2)); }
watchdog_ms() { # scenario [close_ms]
    local w s=$STEP_TIMEOUT_MS windows steps extra=0
    case $1 in
        idle) windows=1 steps=6 ;;
        animate) windows=2 steps=10 ;;
        cdp) windows=1 steps=12 ;;
        devtools) windows=2 steps=14 extra=$2 ;;
        *) echo "no watchdog for $1" >&2; return 1 ;;
    esac
    # Each step stops the function on overflow (inherit_errexit).
    w=$(add "$WARMUP_MS" "$MEASURE_MS")
    w=$(mul "$windows" "$w")
    s=$(mul "$steps" "$s")
    w=$(add "$w" "$s")
    add "$w" "$extra"
}
ms_to_s() { printf '%d.%03d' $(($1 / 1000)) $(($1 % 1000)); }
# Every budget this invocation needs, before anything starts. An assignment
# stops the script under set -e when its command substitution fails.
# shellcheck disable=SC2034 # budget is assigned only for that stop
for s in "${scenarios[@]}"; do
    case $s in
        idle|animate|cdp) budget=$(watchdog_ms "$s") ;;
        animate-unthrottled) budget=$(watchdog_ms animate) ;;
        devtools)
            : "${DEVTOOLS_CLOSE_MS:?}" "${DEVTOOLS_TRIALS:?}"
            for ms in $DEVTOOLS_CLOSE_MS; do
                ms=$(decimal_ms DEVTOOLS_CLOSE_MS "$ms")
                budget=$(watchdog_ms devtools "$ms")
            done ;;
        *) echo "unknown scenario $s" >&2; exit 2 ;;
    esac
done
unset budget

here=$(cd "$(dirname "$0")/.." && pwd)
profile_dir=$(cd "$here/../../target/release" && pwd)
pages=$here/pages
mkdir -p "$results"
results=$(cd "$results" && pwd)

# The workspace cef-rs builds against the beta runtime that
# spikes/gpui-cef-compositor/prepare.sh stages and checks against pins.toml.
# The patched cef-rs build script does not copy that runtime on Linux, so stage
# the binary in a bundle next to a copy of it.
# shellcheck source=../../gpui-cef-compositor/runtime-paths.sh
source "$here/../gpui-cef-compositor/runtime-paths.sh"
runtime_paths linux64 cef_linux_x86_64
check_runtime
cef_dir=$runtime
# One bundle per archive, so files from another CEF build never mix in.
bundle=$profile_dir/cef-gpui-pump-bundle/${archive_name%.tar.bz2}
mkdir -p "$bundle/locales"
find "$cef_dir" -maxdepth 1 -type f -exec cp --reflink=auto -f -t "$bundle" {} +
cp --reflink=auto -f -t "$bundle/locales" "$cef_dir"/locales/*
cp --reflink=auto -f "$profile_dir/cef-gpui-pump" "$bundle/"
bin=$bundle/cef-gpui-pump
echo "bundle $bundle (CEF from $cef_dir)"
# What this run tested: the commit, any tracked edits on top of it, and the CEF archive.
# A copy without git metadata (for example from `git archive`) still runs.
if head=$(git -C "$here" rev-parse HEAD 2>/dev/null); then
    {
        echo "head $head"
        echo "tracked changes in rewrite/:"
        git -C "$here/../.." status --porcelain --untracked-files=no -- .
    } >"$results/build.txt"
else
    echo "no git metadata" >"$results/build.txt"
fi
cp -f "$cef_dir/archive.json" "$results/cef-archive.json"

# The private bus must never take over an existing socket path.
"$here/scripts/check-bus-listener.sh"

# What the app must not reach: the caller's runtime dir, the system bus and
# the caller's home. What it needs from that home: the bundle with libcef,
# the test pages, and the results dir that holds its stderr.
desktop_runtime=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}
system_bus_socket=/run/dbus/system_bus_socket
real_home=${HOME:?}
# The session's whole PATH: system dirs only, none under the caller's home.
# dbus-run-session is resolved here, and finds dbus-daemon on that PATH.
session_path=/usr/bin:/bin
dbus_run_session=$(command -v dbus-run-session) || { echo "no dbus-run-session on PATH" >&2; exit 2; }
allowed="$bundle:$pages:$results"

# shellcheck source=sweep.sh
source "$here/scripts/sweep.sh"
xvfb_pid=
active_group=
active_runtime=
scratch=()
# An EXIT trap's last status becomes the script's, so this keeps the status it
# was called with.
cleanup() {
    local status=$? d
    # A session's launcher is timeout, which calls setpgid(0, 0) before it
    # forks anything (coreutils src/timeout.c). stop_run kills it by PID first,
    # which stops a launch whose group does not exist yet, then sweeps the
    # group and the run's marker. `jobs -p` lists the launcher even before
    # run_one records it.
    stop_run "$bin" "$xvfb_pid" "$active_group" "$active_runtime" >/dev/null
    if [[ -n $xvfb_pid ]] && kill -0 "$xvfb_pid" 2>/dev/null; then
        kill "$xvfb_pid"
        wait "$xvfb_pid" 2>/dev/null || true
    fi
    for d in "${scratch[@]}"; do
        if [[ -d $d ]]; then rm -rf -- "$d"; fi
    done
    exit "$status"
}
trap cleanup EXIT
# bash runs the EXIT trap when it exits; these make a signal end it that way.
trap 'exit 130' INT
trap 'exit 143' TERM

# -displayfd picks a free display number and writes it once the server is ready.
# The shell holds the pipe's write end too, so a dead Xvfb would never give EOF;
# the read is bounded by STEP_TIMEOUT_MS.
exec {dfd}<> <(:)
Xvfb -displayfd "$dfd" -screen 0 1920x1200x24 -nolisten tcp >"$results/xvfb.log" 2>&1 &
xvfb_pid=$!
if ! read -r -t "$(( (STEP_TIMEOUT_MS + 999) / 1000 ))" -u "$dfd" display_number || [[ -z $display_number ]]; then
    echo "Xvfb gave no display within ${STEP_TIMEOUT_MS} ms; see $results/xvfb.log" >&2
    exit 1
fi
exec {dfd}>&-
display=:$display_number
echo "Xvfb pid $xvfb_pid on $display"


failures=0
run_one() {
    local name=$1 url=$2 watchdog=$3
    shift 3
    local out=$results/$name.json
    # A crash must not leave an older report or marker to be read as this run's.
    rm -f -- "$out" "${out%.json}.png" "$results/$name.watchdog.txt" "$results/$name.leftover.txt"
    # One throwaway base per run (mktemp -d makes it mode 700) holds the CEF
    # profile, XDG_RUNTIME_DIR, HOME and TMPDIR; never the owner's profiles,
    # session runtime dir, home or shared /tmp.
    local base cache runtime home tmp
    # Under /tmp, not the caller's TMPDIR, so the path the bus config and its
    # address embed holds only characters sed, XML and D-Bus take literally.
    base=$(mktemp -d -p /tmp cef-gpui-pump-run.XXXXXX)
    scratch+=("$base")
    [[ $base =~ ^/tmp/cef-gpui-pump-run\.[A-Za-z0-9]+$ ]] || { echo "unexpected scratch path $base" >&2; exit 1; }
    cache=$base/cache runtime=$base/runtime home=$base/home tmp=$base/tmp
    mkdir -m 700 -- "$cache" "$runtime" "$home" "$tmp"
    # The bus config names this run's scratch dir; dbus-daemon picks a new socket name in it.
    sed "s|__SCRATCH_RUNTIME_DIR__|$runtime|" "$here/scripts/isolated-session-bus.conf.in" >"$runtime/session-bus.conf"
    echo "== $name (watchdog $(ms_to_s "$watchdog") s)"
    local status=0 session
    active_runtime=$runtime
    # timeout makes itself the leader of a new process group, so the group ID
    # is its PID, and runs the session in that group: dbus-run-session, its
    # dbus-daemon, the browser and every Chromium child
    # (results/2026-11-23-r6-controls/watchdog/tree-before-stop.txt). On expiry
    # it signals the group, then sends SIGKILL after one more STEP_TIMEOUT_MS
    # if dbus-run-session is still running. It waits only for dbus-run-session,
    # so end_group (scripts/sweep.sh) removes whatever else is left, and fails
    # the run if its inventory fails.
    #
    # The session starts from an empty environment (`env -i`) plus what the
    # runner sets itself, so nothing of the caller's session reaches CEF: no
    # SSH_AUTH_SOCK, AT_SPI_BUS_ADDRESS, PULSE_SERVER, XAUTHORITY, Wayland or
    # XDG base dir pointing into the real home or runtime dir. dbus-run-session
    # adds the private bus address. XDG_RUNTIME_DIR is also the run's marker.
    timeout --kill-after="$(ms_to_s "$STEP_TIMEOUT_MS")" "$(ms_to_s "$watchdog")" \
    env -i PATH="$session_path" LANG=C.UTF-8 \
        DISPLAY="$display" XDG_SESSION_TYPE=x11 XDG_RUNTIME_DIR="$runtime" \
        HOME="$home" TMPDIR="$tmp" \
        DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent \
        "$dbus_run_session" --config-file="$runtime/session-bus.conf" -- "$bin" \
        --ozone-platform=x11 --password-store=basic \
        --pump-url="$url" --pump-out="$out" --pump-cache-dir="$cache" \
        --pump-warmup-ms="$WARMUP_MS" --pump-measure-ms="$MEASURE_MS" \
        --pump-step-timeout-ms="$STEP_TIMEOUT_MS" \
        --pump-forbid-paths="$desktop_runtime:$system_bus_socket:$real_home" \
        --pump-allow-paths="$allowed" \
        ${EXTRA_ARGS:-} "$@" >/dev/null 2>"$results/$name.stderr" &
    session=$!
    active_group=$session
    wait "$session" || status=$?
    local expired='' left=0
    if (( status == 124 || status == 137 )); then
        expired=yes
        end_group "$bin" "$session" "$runtime" >"$results/$name.leftover.txt" || left=$?
        echo "watchdog expired after $(ms_to_s "$watchdog") s (exit $status); killed process group $session" | tee "$results/$name.watchdog.txt"
    fi
    # What landed in the scratch dirs would otherwise have gone to the desktop
    # session's runtime dir, the owner's home or the shared /tmp.
    (cd "$base" && find runtime home tmp -mindepth 1 -printf '%y %m %p\n' | sort) >"$results/$name.scratch-dirs.txt"
    # Zygotes exit once the browser process has closed their sockets. Wait for
    # every child the app saw before CefShutdown to exit (pidfd, bounded by
    # STEP_TIMEOUT_MS). This mode reads /proc only and never starts CEF.
    local verdict=fail
    if [[ -n $expired ]]; then
        echo "   exit=$status, killed by the watchdog"
    elif [[ -f $out ]]; then
        env -i HOME="$home" TMPDIR="$tmp" PATH="$PATH" "$bin" --pump-wait-gone="$out" || true
        [[ $status -eq 0 && $(jq .pass "$out") == true ]] && verdict=pass
        echo "   exit=$status scenario_pass=$(jq .scenario_pass "$out") pump_pass=$(jq .pump_pass "$out") isolation_clean=$(jq .isolation_clean "$out") shutdown_clean=$(jq .shutdown_clean "$out") pass=$(jq .pass "$out")"
    else
        echo "   exit=$status, no report written (see $name.stderr)"
    fi
    # After the leak check, so a child slow to exit is reported as a leak
    # rather than killed first: nothing of the session may outlive it.
    if [[ -z $expired ]]; then
        end_group "$bin" "$session" "$runtime" >"$results/$name.leftover.txt" || left=$?
        if (( left == 1 || left == 2 )); then
            verdict=fail
            echo "   session processes outlived the session; killed process group $session (see $name.leftover.txt)"
        fi
    fi
    case $left in
        2) verdict=fail; echo "   process group $session survived SIGKILL (see $name.leftover.txt)" ;;
        3) verdict=fail; echo "   the sweep of process group $session and its marked processes was incomplete (see $name.leftover.txt)" ;;
    esac
    active_group='' active_runtime=''
    [[ -s $results/$name.leftover.txt ]] || rm -f -- "$results/$name.leftover.txt"
    rm -rf -- "$base"
    [[ $verdict == pass ]] || failures=$((failures + 1))
}

for s in "${scenarios[@]}"; do
    case $s in
        idle) run_one idle "file://$pages/static.html" "$(watchdog_ms idle)" --pump-scenario=idle ;;
        animate) run_one animate "file://$pages/anim.html" "$(watchdog_ms animate)" --pump-scenario=animate ;;
        cdp) run_one cdp "file://$pages/anim.html" "$(watchdog_ms cdp)" --pump-scenario=cdp ;;
        animate-unthrottled)
            run_one animate-unthrottled "file://$pages/anim.html" "$(watchdog_ms animate)" \
                --pump-scenario=animate --pump-no-inactive-throttle ;;
        devtools)
            : "${DEVTOOLS_CLOSE_MS:?}" "${DEVTOOLS_TRIALS:?}"
            for ms in $DEVTOOLS_CLOSE_MS; do
                ms=$(decimal_ms DEVTOOLS_CLOSE_MS "$ms")
                for i in $(seq 1 "$DEVTOOLS_TRIALS"); do
                    run_one "devtools-${ms}ms-$i" "file://$pages/anim.html" "$(watchdog_ms devtools "$ms")" --pump-scenario=devtools \
                        --pump-devtools-close-ms="$ms" --pump-second-browser
                done
            done ;;
        *) echo "unknown scenario $s" >&2; exit 2 ;;
    esac
done

if (( failures )); then
    echo "$failures run(s) failed" >&2
    exit 1
fi
echo "all runs passed"
