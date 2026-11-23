#!/usr/bin/env bash
set -euo pipefail

source "$(dirname -- "${BASH_SOURCE[0]}")/runtime-paths.sh"

prepare_source() {
  local pin_table=$1 name=$2 repo rev destination patch_file
  repo=$(pin "$pin_table" git)
  rev=$(pin "$pin_table" rev)
  destination="$sources/$name"
  patch_file="$spike/patches/$name.patch"
  if [[ ! -d $destination ]]; then
    staging="$temporary/$name"
    mkdir "$staging"
    curl -fsSL "https://codeload.github.com/${repo#https://github.com/}/tar.gz/$rev" |
      tar -xz --strip-components=1 -C "$staging"
    patch -d "$staging" -p1 --batch --forward < "$patch_file"
    printf '%s\n%s\n' "$repo" "$rev" > "$staging/.flecha-revision"
    mv "$staging" "$destination"
  elif cmp -s "$destination/.flecha-revision" <(printf '%s\n%s\n' "$repo" "$rev") &&
    patch -d "$destination" -p1 --dry-run --batch --reverse < "$patch_file" >/dev/null; then
    printf '%s patches already applied\n' "$name"
  else
    printf 'Existing %s does not match the pinned repository, revision or patch. Use a fresh target directory.\n' "$destination" >&2
    return 1
  fi
}

case "${1:-$(uname -s):$(uname -m)}" in
  linux64|Linux:x86_64) platform=linux64; directory=cef_linux_x86_64 ;;
  macosarm64|Darwin:arm64) platform=macosarm64; directory=cef_macos_aarch64 ;;
  windows64|MINGW*:x86_64) platform=windows64; directory=cef_windows_x86_64 ;;
  *) printf 'Specify linux64, macosarm64 or windows64\n' >&2; exit 2 ;;
esac

mkdir -p "$sources/cef-beta"
lock="$sources/.prepare-lock"
if ! mkdir "$lock"; then
  printf 'Preparation already owns %s\n' "$lock" >&2
  exit 1
fi
temporary=
cleanup() {
  if [[ -n $temporary ]]; then rm -rf -- "$temporary"; fi
  rmdir "$lock"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
temporary=$(mktemp -d "$sources/prepare.XXXXXX")

prepare_source gpui zed
prepare_source cef-rs cef-rs

runtime_paths "$platform" "$directory"

verify_archive() {
  if command -v sha1sum >/dev/null; then
    printf '%s  %s\n' "$sha1" "$1" | sha1sum --check
  else
    printf '%s  %s\n' "$sha1" "$1" | shasum -a 1 -c
  fi
}

if [[ ! -f $archive ]]; then
  download="$temporary/$archive_name"
  curl -fsSL "https://cef-builds.spotifycdn.com/${archive_name//+/%2B}" -o "$download"
  verify_archive "$download"
  mv "$download" "$archive"
else
  verify_archive "$archive"
fi

if [[ ! -e $runtime ]]; then
  unpack="$temporary/unpack"
  mkdir "$unpack"
  tar -xf "$archive" --strip-components=1 -C "$unpack"
  release="$unpack/Release"
  for item in CMakeLists.txt CREDITS.html cmake include libcef_dll; do
    mv -- "$unpack/$item" "$release/"
  done
  if [[ $platform != macosarm64 ]]; then
    for item in "$unpack/Resources/"*; do
      mv -- "$item" "$release/"
    done
  fi
  archive_metadata > "$release/archive.json"
  mkdir -p "$runtime_parent"
  mv "$release" "$runtime"
fi
check_runtime
printf 'Prepared %s\n' "$runtime"
