#!/usr/bin/env bash
set -euo pipefail

launcher="$(dirname -- "${BASH_SOURCE[0]}")/isolated-run.sh"
if [[ $# -gt 0 ]]; then
  exec "$launcher" x11 --binary "$1" -- node tools/verify-agent.mjs
fi
exec "$launcher" x11 -- node tools/verify-agent.mjs
