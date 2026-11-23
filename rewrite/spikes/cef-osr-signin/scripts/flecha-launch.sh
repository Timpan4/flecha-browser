#!/usr/bin/env bash
# Starts today's Chromium-fork Flecha for the reference column of auto-checks.sh, inside
# xvfb-isolated.sh (LAUNCHER=<this file>), so it gets the same Xvfb, private session bus and
# scratch HOME, XDG and TMPDIR dirs as the spike.
#
#   FLECHA_DIR=<install> flecha-launch.sh --profile <dir> --url <url> [--fake-camera]
#
# Never the `flecha` launcher and never the owner's profile: the browser gets
# --user-data-dir=<dir> (a scratch profile) and a copy of the install's shell directory, since
# Chromium may write `_metadata` into an unpacked extension directory. The install is only
# read. The VSR video worker (LD_PRELOAD in the launcher) is left out; no check plays video.
#
# Chromium's own log goes to stderr (--enable-logging=stderr). Page console messages, which
# Chromium 158 logs as `[...:INFO:CONSOLE:<line>] "<message>", source: <url> (<line>)`, are
# rewritten to the spike's `[console] <message> (<url>:<line>)` form, so the same ACTIONS
# scripts read both. A `[load-end] flecha` line is printed at start for xvfb-isolated.sh,
# which waits for one before it runs ACTIONS; the scripts wait for the window themselves.
set -euo pipefail
: "${FLECHA_DIR:?set FLECHA_DIR to the Flecha install}"
profile='' url=about:blank fake_camera=0
while (($#)); do
    case $1 in
        --profile)
            profile=$2
            shift 2
            ;;
        --url)
            url=$2
            shift 2
            ;;
        --fake-camera)
            fake_camera=1
            shift
            ;;
        *)
            echo "flecha-launch.sh: unknown argument $1" >&2
            exit 2
            ;;
    esac
done
[[ $profile == /* ]] || {
    echo 'flecha-launch.sh: --profile <absolute dir> is required' >&2
    exit 2
}
# The spike's binary limits --fake-camera to local pages; Flecha cannot, so this launcher
# refuses any other start URL. Pages can still navigate on their own: the reference runs only
# open scripts/input-test.html, and no profile here holds an account.
if ((fake_camera)) &&
    ! [[ $url == file://* || $url =~ ^https?://(127\.0\.0\.1|localhost|\[::1\])(:[0-9]+)?(/|$) ]]; then
    echo "flecha-launch.sh: --fake-camera needs a file:// or loopback --url" >&2
    exit 2
fi
mkdir -p "$profile"
shell=$profile.shell
rm -rf -- "$shell"
cp -a "$FLECHA_DIR/shell" "$shell"
args=(
    --user-data-dir="$profile" --flecha-shell-dir="$shell" --class=flecha
    --no-first-run --no-default-browser-check --ozone-platform=x11
    --password-store=basic --enable-logging=stderr
)
((fake_camera)) && args+=(--use-fake-device-for-media-stream)
echo '[load-end] flecha' >&2
"$FLECHA_DIR/chrome" "${args[@]}" "$url" 2>&1 >/dev/null |
    sed -u -E 's/^\[[^]]*:INFO:CONSOLE:([0-9]+)\] "(.*)", source: (.*) \([0-9]+\)$/[console] \2 (\3:\1)/' >&2
