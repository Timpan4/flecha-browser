#!/usr/bin/env bash
# Runs the REPORT checklist rows that need no person, on Xvfb only, and writes a results table
# in the REPORT's shape with a build stamp.
#
#   WAIT_SECS=<secs> auto-checks.sh <results.md> [osr] [chrome] [flecha]
#
# Modes default to osr and chrome. flecha drives today's Chromium-fork Flecha (FLECHA_DIR,
# default ~/.local/opt/flecha-current) as the reference column, through flecha-launch.sh with a
# scratch profile; it never starts the `flecha` launcher and never touches the install or any
# real profile.
#
# Every launch goes through xvfb-isolated.sh: its own Xvfb, a private session bus without
# service activation, a system bus address that points nowhere, scratch HOME, XDG and runtime
# dirs, and a scratch profile. Every run passes --password-store=basic, so no keyring is
# reachable. Checks 1 to 3 (Google sign-in) are never automated.
#
# CASES=<regex> runs only the cases whose names match (for reruns of one check); the table then
# says "not run" or "inconclusive" for the rest.
#
# Each pass condition also runs as a negative control with its cause removed (CONTROL=1, or
# without --fake-camera); the script exits 1 if any control passes, if a guard row does not
# pass, if a row stayed inconclusive (a run ended before observing it, or its precondition was
# not met), if any process group it started is still alive afterwards, or if its scratch
# directory remains.
#
# Needs: Xvfb, xdotool (XDOTOOL), xclip (XCLIP), setxkbmap, ImageMagick (import, convert),
# pdftotext, sqlite3, node, dbus-run-session. WAIT_SECS bounds each wait in a check; it is a
# test bound, not a product value.
set -euo pipefail
: "${WAIT_SECS:?set WAIT_SECS, the per-step wait in seconds}"
here=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
spike=$(cd -- "$here/.." && pwd)
rewrite=$(cd -- "$spike/../.." && pwd)
results=$(realpath -m -- "${1:?usage: auto-checks.sh <results.md> [osr] [chrome] [flecha]}")
shift
modes=("$@")
((${#modes[@]})) || modes=(osr chrome)
for mode in "${modes[@]}"; do
    [[ $mode == @(osr|chrome|flecha) ]] || {
        echo "unknown mode: $mode" >&2
        exit 2
    }
done
XDOTOOL=${XDOTOOL:-$(command -v xdotool)}
XCLIP=${XCLIP:-$(command -v xclip)}
FLECHA_DIR=${FLECHA_DIR:-$HOME/.local/opt/flecha-current}
export WAIT_SECS XDOTOOL XCLIP

input_url=file://$here/input-test.html
print_url=file://$here/print-test.html
store_url=https://chromewebstore.google.com/detail/dark-reader/eimadpbcbfnmbkopoojfekhnkhdbieeh
login_url=https://the-internet.herokuapp.com/login

# Short names: Chromium's singleton socket must fit in 107 bytes (see xvfb-isolated.sh).
scratch=$(mktemp -d "${TMPDIR:-/tmp}/fac.XXXXXXXX")
cells=$scratch/cells.tsv
evidence=$scratch/evidence
mkdir -p "$evidence"
: >"$cells"
leftovers=0
groups=()
declare -A dir_of
cases=0

# shellcheck disable=SC2329 # runs through trap EXIT
finish() {
    local pgid
    for pgid in "${groups[@]}"; do
        if kill -0 -- "-$pgid" 2>/dev/null; then
            echo "process group $pgid still running; stopping it" >&2
            kill -KILL -- "-$pgid" 2>/dev/null || true
        fi
    done
    rm -rf -- "$scratch"
}
trap finish EXIT

# run_case <name> <mode> <rows> <actions> <env assignments...> -- <spike arguments...>
# Runs one launch through xvfb-isolated.sh in its own process group and collects its CELL
# lines. A row the actions did not report (the run failed first) becomes "inconclusive".
run_case() {
    local name=$1 mode=$2 rows=$3 actions=$4 out status=0 row pgid
    shift 4
    [[ $name =~ ${CASES:-.} ]] || return 0
    cases=$((cases + 1))
    out=$scratch/c$cases
    dir_of[$name]=$out
    local env=()
    while [[ $1 != -- ]]; do
        env+=("$1")
        shift
    done
    shift
    local launch=(--mode "$mode" --profile "$out/profile" "$@" -- --password-store=basic)
    if [[ $mode == flecha ]]; then
        env+=(LAUNCHER="$here/flecha-launch.sh" FLECHA_DIR="$FLECHA_DIR")
        launch=(--profile "$out/profile" "$@")
    fi
    mkdir -p "$out"
    echo "== $name ($mode)" >&2
    setsid env APP_MODE="$mode" ACTIONS="$here/$actions" "${env[@]}" \
        bash "$here/xvfb-isolated.sh" "$out" "${launch[@]}" >"$out/run.out" 2>&1 &
    pgid=$!
    groups+=("$pgid")
    wait "$pgid" || status=$?
    mapfile -t -O "${#groups[@]}" groups < <(sed -n 's/^xvfb pid \([0-9]*\) .*/\1/p; s/^app pid \([0-9]*\)$/\1/p' "$out/run.out")
    for pgid in "${groups[@]}"; do
        if kill -0 -- "-$pgid" 2>/dev/null; then
            echo "LEFTOVER process group $pgid after $name" >&2
            leftovers=$((leftovers + 1))
        fi
    done
    grep -a "^CELL	" "$out/run.out" | sed "s/\$/	$name/" >>"$cells" || true
    for row in $rows; do
        grep -aqP "^CELL\t$row\t$mode\t.*\t$name\$" "$cells" ||
            printf 'CELL\t%s\t%s\tinconclusive\tthe run ended before this row was observed (exit %s; evidence/%s.log)\t%s\n' \
                "$row" "$mode" "$status" "$name" "$name" >>"$cells"
    done
    # Evidence: the harness output and the app's own lines (not Chromium's internal log
    # lines), with the scratch path and home directory shortened.
    {
        cat "$out/run.out"
        echo "-- app log"
        grep -av '^\[[0-9]\{4,\}/' "$out/stderr.log" 2>/dev/null || true
    } | sed -e "s#$scratch#<scratch>#g" -e "s#$HOME#~#g" >"$evidence/$name.log"
}

# The fake-device guard before anything starts: no display, no bus, empty environment. The
# spike's binary must refuse each Google sign-in origin and any other remote start URL with
# --fake-camera (exit 2, before CEF starts), and so must flecha-launch.sh for Flecha.
guard_cli() {
    local url status out who expect
    for url in https://accounts.google.com/ https://mail.google.com/ https://myaccount.google.com/ \
        https://accounts.youtube.com/ https://example.com/; do
        for who in spike flecha; do
            [[ $who == spike ]] || has_mode flecha || continue
            status=0
            if [[ $who == spike ]]; then
                expect='--fake-camera needs a local --url'
                out=$(env -i HOME="$scratch/cli-home" PATH=/usr/bin:/bin bash "$spike/run.sh" \
                    --mode osr --profile "$scratch/cli-profile" --fake-camera --url "$url" 2>&1) ||
                    status=$?
            else
                expect='--fake-camera needs a file:// or loopback --url'
                out=$(env -i HOME="$scratch/cli-home" PATH=/usr/bin:/bin FLECHA_DIR="$FLECHA_DIR" \
                    bash "$here/flecha-launch.sh" --profile "$scratch/cli-profile" --fake-camera --url "$url" 2>&1) ||
                    status=$?
            fi
            if [[ $status == 2 && $out == *"$expect"* && ! -e $scratch/cli-profile ]]; then
                printf 'CELL\tguard-cli\t%s\tpass\t%s refused before anything started (exit 2, no profile created)\tguard-cli\n' "$who" "$url" >>"$cells"
            else
                printf 'CELL\tguard-cli\t%s\tfail\t%s: exit %s\tguard-cli\n' "$who" "$url" "$status" >>"$cells"
            fi
        done
    done
}

# note <row> <mode> <result> <note>: a cell no run observed, with the reason.
note() { printf 'CELL\t%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$4" "note-$2" >>"$cells"; }
# copy_profile <case>: copies that case's profile into the next case's directory.
copy_profile() {
    local to=$scratch/c$((cases + 1))
    mkdir -p "$to"
    [[ -z ${dir_of[$1]:-} ]] || cp -a "${dir_of[$1]}/profile" "$to/profile"
}
has_mode() { [[ " ${modes[*]} " == *" $1 "* ]]; }

guard_cli
for mode in "${modes[@]}"; do
    run_case "input-$mode" "$mode" '0a 0b' check0-input.sh -- --url "$input_url"
    run_case "input-$mode-control" "$mode" '0a 0b' check0-input.sh CONTROL=1 -- --url "$input_url"
    run_case "camera-$mode" "$mode" 6 check6-camera.sh -- --url "$input_url" --fake-camera
    run_case "camera-$mode-control" "$mode" 6 check6-camera.sh CONTROL=1 -- --url "$input_url"
    run_case "print-$mode" "$mode" 7 check7-print.sh -- --url "$print_url"
    run_case "print-$mode-control" "$mode" 7 check7-print.sh CONTROL=1 -- --url "$input_url"
    if [[ $mode == flecha ]]; then
        why='Flecha has no page probe (the spike'"'"'s --probe), which this step needs to'
        note 4a flecha 'not automated' "$why find and focus the store button"
        note 4b flecha 'not automated' "needs Dark Reader installed, and 4a is not automated here (REPORT says what was tried)"
        note 4c flecha 'not automated' "the popup opens from the toolbar's extension menu, Chrome's own UI"
        note 5a flecha 'not automated' "$why focus the login fields and see the secure page"
        note 5b flecha 'not automated' "$why read the login fields' lengths"
        continue
    fi
    run_case "store-$mode" "$mode" '4a 4b 4c' check4-store.sh -- --url "$store_url" --probe
    run_case "store-$mode-control" "$mode" '4a 4b' check4-store.sh CONTROL=1 -- --url "$store_url" --probe
    run_case "guard-$mode" "$mode" guard fake-camera-guard.sh GUARD=on -- --url "$input_url" --fake-camera
    run_case "guard-$mode-control" "$mode" guard-control fake-camera-guard.sh GUARD=off -- --url "$input_url"
done
# The content script in OSR: Alloy cannot install from the store (check 4a), so 4b runs on a
# copy of the Chrome-style profile that installed Dark Reader (same Chromium profile format).
if has_mode osr && [[ -n ${dir_of[store-chrome]:-} ]]; then
    copy_profile store-chrome
    run_case content-osr osr 4b check4-store.sh PHASE=content -- --url "$input_url" --probe
fi
# Password save, then fill on the saved profile. OSR saves nothing, so its fill runs on a
# copy of the Chrome-style profile that saved the login.
for mode in "${modes[@]}"; do
    [[ $mode != flecha ]] || continue
    run_case "password-save-$mode" "$mode" 5a check5-password.sh PHASE=save -- --url "$login_url" --probe
    run_case "password-save-$mode-control" "$mode" 5a check5-password.sh PHASE=save CONTROL=1 -- --url "$login_url" --probe
done
for mode in "${modes[@]}"; do
    [[ $mode != flecha ]] || continue
    source=$mode note=''
    if [[ $mode == osr && -n ${dir_of[password-save-chrome]:-} ]]; then
        source=chrome note='a copy of the Chrome-style profile that saved it'
    fi
    copy_profile "password-save-$source"
    run_case "password-fill-$mode" "$mode" 5b check5-password.sh PHASE=fill PROFILE_NOTE="$note" -- --url "$login_url" --probe
    run_case "password-fill-$mode-control" "$mode" 5b check5-password.sh PHASE=fill CONTROL=1 -- --url "$login_url" --probe
done

# ---- results ----
cell_of() { # <row> <mode> <case>: "result: note" for one cell
    awk -F'\t' -v r="$1" -v m="$2" -v c="$3" '$2 == r && $3 == m && $6 == c { print $4 ": " $5; exit }' "$cells"
}
column() { # <row> <mode> <case>: the case's cell, else a noted one
    local mode=$2 text
    has_mode "$mode" || {
        echo 'not run'
        return
    }
    text=$(cell_of "$1" "$mode" "$3")
    [[ -n $text ]] || text=$(cell_of "$1" "$mode" "note-$mode")
    printf '%s\n' "$text" | sed 's/|/\\|/g'
}
row0() { # typing and paste merged into the REPORT's row 0
    local mode=$1 a b
    [[ " ${modes[*]} " == *" $mode "* ]] || {
        echo 'not run'
        return
    }
    a=$(cell_of 0a "$mode" "input-$mode") b=$(cell_of 0b "$mode" "input-$mode")
    printf 'typing %s; paste %s' "$a" "$b" | sed 's/|/\\|/g'
}

dirty=$(git -C "$rewrite" status --porcelain | grep -c '' || true)
bin=${FLECHA_SPIKE_BIN:-$rewrite/target/debug/cef-osr-signin}
{
    echo "# Spike 0 automated checks"
    echo
    echo "Written by \`scripts/auto-checks.sh\` (modes: ${modes[*]}). Every cell was observed on Xvfb through \`scripts/xvfb-isolated.sh\`; its label says how. Checks 1 to 3 need a person and a Google account and are never automated."
    echo
    echo "## Build stamp"
    echo
    echo "| Item | Value |"
    echo "|---|---|"
    echo "| Date (UTC) | $(date -u +%Y-%m-%dT%H:%M:%SZ) |"
    echo "| Commit | \`$(git -C "$rewrite" rev-parse HEAD)\`$( ((dirty)) && echo ", $dirty uncommitted paths" || echo ', clean tree') |"
    echo "| Spike binary sha256 | \`$(sha256sum "$bin" | cut -d' ' -f1)\` |"
    # shellcheck source=cef-pins.sh
    echo "| CEF pin | \`$(. "$here/cef-pins.sh" && echo "$cef_version") $(. "$here/cef-pins.sh" && echo "$cef_channel")\` |"
    if [[ " ${modes[*]} " == *' flecha '* ]]; then
        echo "| Flecha | \`$("$FLECHA_DIR/chrome" --version 2>/dev/null || echo unknown)\`, chrome sha256 \`$(sha256sum "$FLECHA_DIR/chrome" | cut -c1-16)…\` |"
    fi
    echo "| Password store | \`--password-store=basic\` in a scratch HOME on every run; no keyring is reachable (no session bus but the private one, system bus address points nowhere) |"
    echo "| WAIT_SECS | $WAIT_SECS (per-step wait; a test bound) |"
    echo "| Tools | $("$XDOTOOL" --version 2>&1 | sed -n 1p), xclip $("$XCLIP" -version 2>&1 | sed -n 's/^xclip version \([^ ]*\).*/\1/p'), $(Xvfb -version 2>&1 | grep -o 'X.Org X Server [0-9.]*' | sed -n 1p), $(convert -version | sed -n '1s/^Version: \(ImageMagick [^ ]*\).*/\1/p'), $(pdftotext -v 2>&1 | sed -n 1p) |"
    echo
    echo "## Results"
    echo
    echo "| # | Check | Alloy OSR | Chrome-style control | Flecha (reference) |"
    echo "|---|---|---|---|---|"
    echo "| 0 | Typing and paste (Xvfb, xdotool; the owner's real keyboard, IME and Wayland stay manual) | $(row0 osr) | $(row0 chrome) | $(row0 flecha) |"
    echo "| 1 | Gmail sign-in with MFA, survives restart | manual | manual | manual |"
    echo "| 2 | Sign out and back in | manual | manual | manual |"
    echo "| 3 | Third-party \"Sign in with Google\" | manual | manual | manual |"
    osr_4b="$(column 4b osr store-osr)"
    [[ -z $(cell_of 4b osr content-osr) ]] || osr_4b+="; then $(column 4b osr content-osr)"
    echo "| 4a | Web Store: \"Add to Chrome\" enabled and install completes | $(column 4a osr store-osr) | $(column 4a chrome store-chrome) | $(column 4a flecha store-flecha) |"
    echo "| 4b | Extension content script runs | $osr_4b | $(column 4b chrome store-chrome) | $(column 4b flecha note-flecha) |"
    for spec in '4c|Extension popup opens|store' \
        '5a|Password save offered|password-save' '5b|Saved password fills|password-fill' \
        '6|Camera permission through a host prompt (fake device)|camera' '7|Print|print'; do
        IFS='|' read -r row title case <<<"$spec"
        echo "| $row | $title | $(column "$row" osr "$case-osr") | $(column "$row" chrome "$case-chrome") | $(column "$row" flecha "$case-flecha") |"
    done
    echo
    echo "## Fake-camera guard"
    echo
    echo "| Run | Result |"
    echo "|---|---|"
    awk -F'\t' '$2 ~ /^guard/ { gsub(/\|/, "\\|", $5); print "| " $6 " (" $3 ") | " $4 ": " $5 " |" }' "$cells"
    echo
    echo "## Negative controls"
    echo
    echo "Each control runs the same pass condition with its cause removed. A control that passes would mean the check can pass when it should not; none may."
    echo
    echo "| Control run | Row | Result |"
    echo "|---|---|---|"
    awk -F'\t' '$6 ~ /-control$/ && $2 !~ /^guard/ { gsub(/\|/, "\\|", $5); print "| " $6 " | " $2 " | " $4 ": " $5 " |" }' "$cells"
    echo "| empty PDF file | 7 | $(: >"$scratch/empty.pdf"; APP_MODE=osr OUT=$scratch bash -c '. "$1/auto-lib.sh"; pdf_has_phrase "$2"' _ "$here" "$scratch/empty.pdf" 2>/dev/null && echo 'pass (WRONG)' || echo 'did not pass') |"
} >"$results.tmp"

# A control "passes" when its pass condition held; the guard control is the reverse (it must
# reach 127.0.0.2, so its own pass is expected).
bad_controls=$(awk -F'\t' '$6 ~ /-control$/ && $2 !~ /^guard/ && $4 == "pass"' "$cells" | grep -c '' || true)
grep -c 'pass (WRONG)' "$results.tmp" >/dev/null && bad_controls=$((bad_controls + 1))
# The guard rows (binary refusals, guarded runs and their controls) must all pass.
guard_fails=$(awk -F'\t' '$2 ~ /^guard/ && $4 != "pass"' "$cells" | grep -c '' || true)
# A row a run should have observed but did not (the run failed before it).
unobserved=$(awk -F'\t' '$4 == "inconclusive"' "$cells" | grep -c '' || true)
mkdir -p "$results.evidence"
cp "$evidence"/*.log "$results.evidence/"
mv "$results.tmp" "$results"
echo "results: $results" >&2
echo "controls that passed: $bad_controls; guard rows not passing: $guard_fails; unobserved rows: $unobserved; leftover process groups: $leftovers" >&2
trap - EXIT
finish
if [[ -e $scratch ]]; then
    echo "scratch directory $scratch remains" >&2
    exit 1
fi
echo "scratch directory removed" >&2
((bad_controls == 0 && guard_fails == 0 && unobserved == 0 && leftovers == 0))
