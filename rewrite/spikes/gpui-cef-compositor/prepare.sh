#!/usr/bin/env bash
set -euo pipefail

source "$(dirname -- "${BASH_SOURCE[0]}")/runtime-paths.sh"

prepare_source() {
  local pin_table=$1 name=$2 repo rev destination tree patch_file
  repo=$(pin "$pin_table" git)
  rev=$(pin "$pin_table" rev)
  destination="$sources/$name"
  tree="$destination"
  patch_file="$spike/patches/$name.patch"
  if [[ ! -d $destination ]]; then
    tree="$temporary/$name"
    mkdir "$tree"
    curl -fsSL "https://codeload.github.com/${repo#https://github.com/}/tar.gz/$rev" |
      tar -xz --strip-components=1 -C "$tree"
    patch -d "$tree" -p1 --batch --forward < "$patch_file"
    printf '%s\n%s\n' "$repo" "$rev" > "$tree/.flecha-revision"
  # Force the reverse direction even when a hunk matches only the unpatched tree.
  elif cmp -s "$destination/.flecha-revision" <(printf '%s\n%s\n' "$repo" "$rev") &&
    patch -d "$destination" -p1 --dry-run --force --reverse < "$patch_file" >/dev/null; then
    printf '%s patches already applied\n' "$name"
  else
    printf 'Existing %s does not match the pinned repository, revision or patch. Move aside %s and rerun prepare.sh.\n' "$destination" "$destination" >&2
    return 1
  fi
  if [[ $name == zed ]]; then
    # rustup uses the nearest toolchain file above the working directory, and Zed has nested ones.
    find "$tree" \( -name rust-toolchain -o -name rust-toolchain.toml \) ! -type d -delete
  fi
  if [[ $tree != "$destination" ]]; then
    mv "$tree" "$destination"
  fi
}

sources_only=false
if [[ ${1:-} == --sources-only ]]; then
  sources_only=true
else
  case "${1:-$(uname -s):$(uname -m)}" in
    linux64|Linux:x86_64) platform=linux64; directory=cef_linux_x86_64 ;;
    macosarm64|Darwin:arm64) platform=macosarm64; directory=cef_macos_aarch64 ;;
    windows64|MINGW*:x86_64) platform=windows64; directory=cef_windows_x86_64 ;;
    *) printf 'Specify linux64, macosarm64, windows64 or --sources-only\n' >&2; exit 2 ;;
  esac
fi

mkdir -p "$sources"
if ! command -v flock >/dev/null; then
  printf 'prepare.sh requires flock to serialize staging.\n' >&2
  exit 1
fi
exec 9>"$sources/.prepare.lock"
flock 9
temporary=
cleanup() {
  if [[ -n $temporary ]]; then rm -rf -- "$temporary"; fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
temporary=$(mktemp -d "$sources/prepare.XXXXXX")

prepare_source gpui zed
prepare_source cef-rs cef-rs
if [[ $sources_only == true ]]; then
  exit 0
fi

mkdir -p "$sources/cef-beta"
runtime_paths "$platform" "$directory"
archive="$sources/cef-beta/$archive_name"

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
