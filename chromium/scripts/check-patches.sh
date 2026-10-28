#!/bin/bash
# Checks that every patch still applies, in order, to the Chromium in chromium/CHROMIUM_VERSION,
# without a checkout: only the files the patches change are fetched, into a scratch repo. CI
# runs this; it needs curl and git, not the build volume.
#
# The files come from Chromium's GitHub mirror, which has the same bytes at each release tag.
# googlesource's Gitiles answered GitHub's runners with 504s and bursts of fetches with 429s.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
version="$(tr -d '[:space:]' <"$here/CHROMIUM_VERSION")"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
cd "$work"
git init -q

# Files a patch creates (--- /dev/null) aren't in Chromium, even when later patches change
# them; every other file a patch changes must be.
created="$(awk '/^--- \/dev\/null/ { getline; sub(/^\+\+\+ b\//, ""); print }' "$here"/patches/*.patch | sort -u)"
files="$(sed -n 's|^--- a/||p' "$here"/patches/*.patch | sort -u | comm -23 - <(echo "$created"))"
echo "fetching $(wc -l <<<"$files" | tr -d ' ') files from Chromium $version"
# One curl fetches them all in parallel.
for f in $files; do
  printf 'url = "https://raw.githubusercontent.com/chromium/chromium/%s/%s"\noutput = "%s"\n' "$version" "$f" "$f"
done | curl -fsS --retry 3 --parallel --create-dirs --config -

for p in "$here"/patches/*.patch; do
  echo "patch: $(basename "$p")"
  git apply "$p"
done
echo "all patches apply to Chromium $version"
