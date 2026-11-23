#!/usr/bin/env bash
set -euo pipefail

exec "$(dirname -- "${BASH_SOURCE[0]}")/isolated-run.sh" x11 -- node tools/verify-slice.mjs "$@"
