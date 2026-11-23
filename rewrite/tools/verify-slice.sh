#!/usr/bin/env bash
set -euo pipefail

launcher="$(dirname -- "${BASH_SOURCE[0]}")/isolated-run.sh"
mode=${1:-slice}
if [[ $# -gt 1 ]]; then
  exec "$launcher" x11 --binary "$2" -- node tools/verify-slice.mjs "$mode"
fi
exec "$launcher" x11 -- node tools/verify-slice.mjs "$mode"
