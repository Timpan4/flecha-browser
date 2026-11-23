#!/usr/bin/env bash
# Runs every Spike 2 check on a private Xvfb display, a private D-Bus session
# with no service activation, a scratch XDG_RUNTIME_DIR, HOME and TMPDIR per
# run, and a system bus address that points nowhere. Each app run checks that
# its process tree holds nothing under the desktop session's runtime dir, the
# system bus socket or the caller's home outside the bundle, pages and results
# (`isolation_clean` in the report). Stops what it starts by PID, or by the
# process group ID of a run's session: it kills that group when the run
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

results=${1:?usage: run-checks.sh RESULTS_DIR [SCENARIO...]}
shift
scenarios=("$@")
(( ${#scenarios[@]} )) || scenarios=(idle animate animate-unthrottled cdp devtools)

: "${WARMUP_MS:?set WARMUP_MS}" "${MEASURE_MS:?set MEASURE_MS}" "${STEP_TIMEOUT_MS:?set STEP_TIMEOUT_MS}"
# Bash arithmetic reads 0100 as octal and the app parses decimal, so take digits
# only and use their decimal value: the watchdog then budgets what the app runs.
decimal_ms() { # name value
    [[ $2 =~ ^[0-9]+$ ]] || { echo "$1 must be a whole number of milliseconds, got '$2'" >&2; exit 2; }
    echo $((10#$2))
}
WARMUP_MS=$(decimal_ms WARMUP_MS "$WARMUP_MS")
MEASURE_MS=$(decimal_ms MEASURE_MS "$MEASURE_MS")
STEP_TIMEOUT_MS=$(decimal_ms STEP_TIMEOUT_MS "$STEP_TIMEOUT_MS")
# The app keeps the last value of a repeated flag, so a timing flag in
# EXTRA_ARGS would run with durations the watchdog did not budget.
case " ${EXTRA_ARGS:-} " in
    *--pump-warmup-ms*|*--pump-measure-ms*|*--pump-step-timeout-ms*|*--pump-devtools-close-ms*)
        echo "set timings through WARMUP_MS, MEASURE_MS, STEP_TIMEOUT_MS and DEVTOOLS_CLOSE_MS, not EXTRA_ARGS" >&2
        exit 2 ;;
esac

here=$(cd "$(dirname "$0")/.." && pwd)
profile_dir=$(cd "$here/../../target/release" && pwd)
pages=$here/pages
mkdir -p "$results"
results=$(cd "$results" && pwd)

# The workspace cef-rs builds against the beta runtime that
# spikes/gpui-cef-compositor/prepare.sh stages and checks against pins.toml.
# Cargo 1.100's build-dir layout keeps cef-rs's runtime copy out of the binary's
# dir (tauri-apps/cef-rs#522), so stage the binary next to that runtime.
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
allowed="$bundle:$pages:$results"

xvfb_pid=
active_group=
scratch=()
# An EXIT trap's last status becomes the script's, so this keeps the status it
# was called with.
cleanup() {
    local status=$? d
    if [[ -n $active_group ]]; then end_group "$active_group" >/dev/null || true; fi
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
watchdog_ms() { # scenario [close_ms]
    local w=$((WARMUP_MS + MEASURE_MS)) s=$STEP_TIMEOUT_MS
    case $1 in
        idle) echo $((w + 6 * s)) ;;
        animate) echo $((2 * w + 10 * s)) ;;
        cdp) echo $((w + 12 * s)) ;;
        devtools) echo $((2 * w + 14 * s + $2)) ;;
        *) echo "no watchdog for $1" >&2; return 1 ;;
    esac
}
ms_to_s() { printf '%d.%03d' $(($1 / 1000)) $(($1 % 1000)); }

# The members of process group $1, one `ps` line each.
group_members() { ps -eo pid=,pgid=,stat=,args= | awk -v g="$1" '$2 == g'; }
# Kills what is left of process group $1 by its group ID and waits until the
# group is empty, bounded by STEP_TIMEOUT_MS. Prints the members it found.
# Returns 0 if the group was already empty, 1 if it had members and they are
# gone, 2 if any survived.
end_group() {
    local group=$1 left i
    left=$(group_members "$group")
    [[ -n $left ]] || return 0
    printf '%s\n' "$left"
    kill -KILL -- "-$group" 2>/dev/null || true
    for ((i = 0; i < (STEP_TIMEOUT_MS + 99) / 100; i++)); do
        [[ -n $(group_members "$group") ]] || return 1
        sleep 0.1
    done
    group_members "$group" | sed 's/^/still alive: /'
    return 2
}

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
    # timeout makes itself the leader of a new process group, so the group ID
    # is its PID, and runs the session in that group: dbus-run-session, its
    # dbus-daemon, the browser and every Chromium child
    # (results/2026-11-23-r6-controls/watchdog/tree-before-stop.txt). On expiry
    # it signals the group, then sends SIGKILL after one more STEP_TIMEOUT_MS
    # if dbus-run-session is still running. It waits only for dbus-run-session,
    # so end_group below removes whatever else is left. XDG base dirs and cache
    # overrides would point back into the real home.
    timeout --kill-after="$(ms_to_s "$STEP_TIMEOUT_MS")" "$(ms_to_s "$watchdog")" \
    env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET -u DBUS_SESSION_BUS_ADDRESS \
        -u XDG_CONFIG_HOME -u XDG_CACHE_HOME -u XDG_DATA_HOME -u XDG_STATE_HOME \
        -u MESA_SHADER_CACHE_DIR -u __GL_SHADER_DISK_CACHE_PATH \
        DISPLAY="$display" XDG_SESSION_TYPE=x11 XDG_RUNTIME_DIR="$runtime" \
        HOME="$home" TMPDIR="$tmp" \
        DBUS_SYSTEM_BUS_ADDRESS=unix:path=/nonexistent \
        dbus-run-session --config-file="$runtime/session-bus.conf" -- "$bin" \
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
        end_group "$session" >"$results/$name.leftover.txt" || left=$?
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
        end_group "$session" >"$results/$name.leftover.txt" || left=$?
        if (( left )); then
            verdict=fail
            echo "   session processes outlived the session; killed process group $session (see $name.leftover.txt)"
        fi
    fi
    (( left != 2 )) || { verdict=fail; echo "   process group $session survived SIGKILL (see $name.leftover.txt)"; }
    active_group=
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
