# shellcheck shell=bash
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

runtime_paths() {
  platform=$1
  version=$(pin cef version)
  channel=$(pin cef channel)
  sha1=$(pin "cef.$channel.sha1" "$platform")
  archive_name="cef_binary_${version}_${platform}_${channel}_minimal.tar.bz2"
  runtime_parent="$sources/cef-runtime/${archive_name%.tar.bz2}"
  runtime="$runtime_parent/$2"
}

archive_metadata() {
  printf '{"type":"minimal","name":"%s","sha1":"%s"}\n' "$archive_name" "$sha1"
}

check_runtime() {
  local library
  case "$platform" in
    linux64) library=libcef.so ;;
    windows64) library=libcef.dll ;;
    macosarm64) library='Chromium Embedded Framework.framework/Chromium Embedded Framework' ;;
  esac
  if ! cmp -s "$runtime/archive.json" <(archive_metadata); then
    printf 'Missing or unexpected CEF archive metadata in %s. Move aside that directory if it exists, then run prepare.sh.\n' "$runtime" >&2
    return 1
  fi
  if [[ ! -f $runtime/$library || ! -s $runtime/$library ]]; then
    printf 'Missing or empty CEF library %s. Move aside %s and run prepare.sh.\n' "$runtime/$library" "$runtime" >&2
    return 1
  fi
}
