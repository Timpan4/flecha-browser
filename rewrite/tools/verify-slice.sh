#!/usr/bin/env bash
set -euo pipefail

launcher="$(dirname -- "${BASH_SOURCE[0]}")/isolated-run.sh"
mode=${1:-slice}
if [[ $mode == slice ]] && ! command -v openssl >/dev/null; then
  printf 'OpenSSL is required to generate the slice test certificate.\n' >&2
  exit 1
fi
if [[ $# -gt 1 ]]; then
  exec "$launcher" x11 --binary "$2" -- node tools/verify-slice.mjs "$mode"
fi
exec "$launcher" x11 -- node tools/verify-slice.mjs "$mode"
