#!/usr/bin/env bash
# Runs every Spike 2 check on a private Xvfb display and a private D-Bus session.
# Never touches the desktop session. Stops what it starts by PID.
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

here=$(cd "$(dirname "$0")/.." && pwd)
profile_dir=$(cd "$here/../../target/release" && pwd)
pages=$here/pages
mkdir -p "$results"
results=$(cd "$results" && pwd)

# Cargo 1.100's default build-dir layout makes cef-dll-sys copy CEF's runtime
# files to target/release/build instead of next to the binary (tauri-apps/cef-rs#522).
# Stage the binary with the CEF distribution that cef-dll-sys built against.
want_version=$(awk -F'"' '/^version = / { print $2; exit }' "$here/../../pins.toml")
cef_dir=
for d in "$profile_dir"/build/cef-dll-sys/*/out/cef_linux_x86_64; do
    if [[ -f $d/archive.json ]] && jq -e --arg v "$want_version" '.name | contains($v)' "$d/archive.json" >/dev/null; then
        cef_dir=$d
    fi
done
[[ -n $cef_dir ]] || { echo "no cef-dll-sys output for CEF $want_version; build first" >&2; exit 1; }
bundle=$profile_dir/cef-gpui-pump-bundle
mkdir -p "$bundle/locales"
find "$cef_dir" -maxdepth 1 -type f -exec cp --reflink=auto -u -t "$bundle" {} +
cp --reflink=auto -u -t "$bundle/locales" "$cef_dir"/locales/*
cp --reflink=auto -f "$profile_dir/cef-gpui-pump" "$bundle/"
bin=$bundle/cef-gpui-pump
echo "bundle $bundle (CEF from $cef_dir)"

xvfb_pid=
cleanup() {
    if [[ -n $xvfb_pid ]] && kill -0 "$xvfb_pid" 2>/dev/null; then
        kill "$xvfb_pid"
        wait "$xvfb_pid" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# -displayfd picks a free display number and writes it once the server is ready.
exec {dfd}<> <(:)
Xvfb -displayfd "$dfd" -screen 0 1920x1200x24 -nolisten tcp >"$results/xvfb.log" 2>&1 &
xvfb_pid=$!
read -r -u "$dfd" display_number
exec {dfd}>&-
display=:$display_number
echo "Xvfb pid $xvfb_pid on $display"

run_one() {
    local name=$1 url=$2
    shift 2
    local out=$results/$name.json
    # A fresh, throwaway CEF profile per run; never the owner's profiles.
    local cache
    cache=$(mktemp -d -t cef-gpui-pump-cache.XXXXXX)
    echo "== $name"
    local status=0
    env -u WAYLAND_DISPLAY -u WAYLAND_SOCKET DISPLAY="$display" XDG_SESSION_TYPE=x11 \
        dbus-run-session --config-file="$here/scripts/isolated-session-bus.conf" -- "$bin" \
        --ozone-platform=x11 --password-store=basic \
        --pump-url="$url" --pump-out="$out" --pump-cache-dir="$cache" \
        --pump-warmup-ms="$WARMUP_MS" --pump-measure-ms="$MEASURE_MS" \
        --pump-step-timeout-ms="$STEP_TIMEOUT_MS" \
        ${EXTRA_ARGS:-} "$@" >/dev/null 2>"$results/$name.stderr" || status=$?
    rm -rf -- "$cache"
    # Zygotes exit once the browser process has closed their sockets. Wait for
    # every child the app saw before CefShutdown to exit (pidfd, bounded by STEP_TIMEOUT_MS).
    if [[ -f $out ]]; then
        "$bin" --pump-wait-gone="$out" || true
        echo "   exit=$status scenario_pass=$(jq .scenario_pass "$out") shutdown_clean=$(jq .shutdown_clean "$out") pass=$(jq .pass "$out")"
    else
        echo "   exit=$status, no report written (see $name.stderr)"
    fi
}

for s in "${scenarios[@]}"; do
    case $s in
        idle) run_one idle "file://$pages/static.html" --pump-scenario=idle ;;
        animate) run_one animate "file://$pages/anim.html" --pump-scenario=animate ;;
        cdp) run_one cdp "file://$pages/anim.html" --pump-scenario=cdp ;;
        animate-unthrottled)
            run_one animate-unthrottled "file://$pages/anim.html" --pump-scenario=animate --pump-no-inactive-throttle ;;
        devtools)
            : "${DEVTOOLS_CLOSE_MS:?}" "${DEVTOOLS_TRIALS:?}"
            for ms in $DEVTOOLS_CLOSE_MS; do
                for i in $(seq 1 "$DEVTOOLS_TRIALS"); do
                    run_one "devtools-${ms}ms-$i" "file://$pages/anim.html" --pump-scenario=devtools \
                        --pump-devtools-close-ms="$ms" --pump-second-browser
                done
            done ;;
        *) echo "unknown scenario $s" >&2; exit 2 ;;
    esac
done
