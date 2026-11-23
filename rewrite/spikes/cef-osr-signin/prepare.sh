#!/usr/bin/env bash
# Downloads the CEF archive pinned in rewrite/pins.toml ([cef] version and channel), checks its
# SHA-1 against the pin, and stages it at rewrite/target/cef-runtime/<x.y.z>/cef_linux_x86_64,
# the layout cef-dll-sys accepts as CEF_PATH. Adapted from Spike 1's prepare.sh (Linux only).
set -euo pipefail
# shellcheck source=scripts/cef-pins.sh
. "$(dirname -- "${BASH_SOURCE[0]}")/scripts/cef-pins.sh"

archives="$rewrite/target/cef-archives"
archive="$archives/$cef_archive_name"
mkdir -p "$archives" "$(dirname -- "$cef_runtime")"

if [[ ! -f $archive ]]; then
    download=$(mktemp "$archives/download.XXXXXX")
    curl -fsSL "https://cef-builds.spotifycdn.com/${cef_archive_name//+/%2B}" -o "$download"
    printf '%s  %s\n' "$cef_sha1" "$download" | sha1sum --check --quiet
    mv -T "$download" "$archive"
else
    printf '%s  %s\n' "$cef_sha1" "$archive" | sha1sum --check --quiet
fi

if [[ ! -e $cef_runtime ]]; then
    unpack=$(mktemp -d "$archives/unpack.XXXXXX")
    tar -xf "$archive" --strip-components=1 -C "$unpack"
    release="$unpack/Release"
    for item in CMakeLists.txt CREDITS.html cmake include libcef_dll; do
        mv -- "$unpack/$item" "$release/"
    done
    for item in "$unpack/Resources/"*; do
        mv -- "$item" "$release/"
    done
    printf '%s\n' "$cef_archive_json" >"$release/archive.json"
    mv -T "$release" "$cef_runtime"
    rm -r -- "$unpack"
fi
check_runtime
printf 'CEF_PATH=%s\n' "$cef_path"
