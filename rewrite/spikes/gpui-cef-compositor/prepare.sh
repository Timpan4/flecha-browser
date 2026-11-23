#!/usr/bin/env bash
set -euo pipefail

spike=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
rewrite=$(cd "$spike/../.." && pwd)
sources="$rewrite/target/spike1-src"

pin() {
  local table=$1 key=$2 line active=''
  local value_pattern="^${key}[[:space:]]*=[[:space:]]*\"([^\"]*)\""
  while IFS= read -r line; do
    if [[ $line == \[*\] ]]; then
      active=${line:1:-1}
    elif [[ $active == "$table" && $line =~ $value_pattern ]]; then
      printf '%s\n' "${BASH_REMATCH[1]}"
      return
    fi
  done < "$rewrite/pins.toml"
  printf 'Missing pin %s.%s\n' "$table" "$key" >&2
  return 1
}

prepare_source() {
  local pin_table=$1 name=$2 repo rev staging destination patch_file
  repo=$(pin "$pin_table" git)
  rev=$(pin "$pin_table" rev)
  destination="$sources/$name"
  patch_file="$spike/patches/$name.patch"
  if [[ ! -d $destination ]]; then
    staging=$(mktemp -d "$sources/$name.XXXXXX")
    curl -fsSL "https://codeload.github.com/${repo#https://github.com/}/tar.gz/$rev" |
      tar -xz --strip-components=1 -C "$staging"
    patch -d "$staging" -p1 --batch --forward < "$patch_file"
    mv -T "$staging" "$destination"
  elif patch -d "$destination" -p1 --dry-run --batch --reverse < "$patch_file" >/dev/null; then
    printf '%s patches already applied\n' "$name"
  else
    printf 'Existing %s does not match the patched source. Use a fresh target directory.\n' "$destination" >&2
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
prepare_source gpui zed
prepare_source cef-rs cef-rs

version=$(pin cef version)
channel=$(pin cef channel)
sha1=$(pin "cef.$channel.sha1" "$platform")
archive_name="cef_binary_${version}_${platform}_${channel}_minimal.tar.bz2"
archive="$sources/cef-beta/$archive_name"
runtime_parent="$sources/cef-runtime/${version%%+*}"
runtime="$runtime_parent/$directory"

if [[ ! -f $archive ]]; then
  download=$(mktemp "$sources/cef-beta/cef.XXXXXX")
  curl -fsSL "https://cef-builds.spotifycdn.com/${archive_name//+/%2B}" -o "$download"
  printf '%s  %s\n' "$sha1" "$download" | sha1sum --check
  mv -T "$download" "$archive"
else
  printf '%s  %s\n' "$sha1" "$archive" | sha1sum --check
fi

if [[ ! -e $runtime ]]; then
  unpack=$(mktemp -d "$sources/cef-beta/unpack.XXXXXX")
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
  printf '{"type":"minimal","name":"%s","sha1":"%s"}\n' "$archive_name" "$sha1" > "$release/archive.json"
  mkdir -p "$runtime_parent"
  mv -T "$release" "$runtime"
fi
if ! cmp --silent "$runtime/archive.json" <(
  printf '{"type":"minimal","name":"%s","sha1":"%s"}\n' "$archive_name" "$sha1"
); then
  printf 'Unexpected archive metadata in %s\n' "$runtime" >&2
  exit 1
fi
printf 'Prepared %s\n' "$runtime"
