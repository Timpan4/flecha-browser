# shellcheck shell=bash
# Sourced by prepare.sh, build.sh and run.sh. Reads the CEF pin from rewrite/pins.toml and
# names the staged runtime directory. The pin parser is Spike 1's (prepare.sh there).
spike=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
rewrite=$(cd -- "$spike/../.." && pwd)

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
    done <"$rewrite/pins.toml"
    printf 'Missing pin %s.%s in %s\n' "$table" "$key" "$rewrite/pins.toml" >&2
    return 1
}

cef_version=$(pin cef version)
cef_channel=$(pin cef channel)
cef_sha1=$(pin "cef.$cef_channel.sha1" linux64)
cef_archive_name="cef_binary_${cef_version}_linux64_${cef_channel}_minimal.tar.bz2"
# cef-dll-sys accepts CEF_PATH/<major.minor.patch>/cef_linux_x86_64 (sys/build.rs at the pinned rev).
cef_path="$rewrite/target/cef-runtime"
cef_runtime="$cef_path/${cef_version%%+*}/cef_linux_x86_64"
cef_archive_json=$(printf '{"type":"minimal","name":"%s","sha1":"%s"}' "$cef_archive_name" "$cef_sha1")

# Fails unless the staged runtime exists and its archive.json names the pinned archive.
check_runtime() {
    if [[ ! -f $cef_runtime/libcef.so || ! -f $cef_runtime/archive.json ]]; then
        printf 'No CEF runtime at %s. Run spikes/cef-osr-signin/prepare.sh first.\n' "$cef_runtime" >&2
        return 1
    fi
    if [[ $(<"$cef_runtime/archive.json") != "$cef_archive_json" ]]; then
        printf '%s/archive.json does not name the pinned archive %s (SHA-1 %s). Run prepare.sh in a clean target directory.\n' \
            "$cef_runtime" "$cef_archive_name" "$cef_sha1" >&2
        return 1
    fi
}
