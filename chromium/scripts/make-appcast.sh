#!/bin/bash
# Makes the Sparkle update feed for a release: given the notarized dmg from
#   FLECHA_SIGN_IDENTITY=... FLECHA_NOTARY_PROFILE=... chromium/scripts/package-release.sh --dmg
# it fills a folder to publish on flecha.dev as it is: the dmg and appcast.xml, the feed the
# app reads (SUFeedURL https://flecha.dev/appcast.xml, patch 0053), with the dmg's EdDSA
# signature, made by Sparkle's generate_appcast with the private key in the login Keychain
# (the one generate_keys -p shows the public half of, the app's SUPublicEDKey).
#
#   chromium/scripts/make-appcast.sh <Flecha-<version>.dmg> [<folder>]
#
# The folder ($FLECHA_RELEASE_DIR/appcast by default, next to the dmg) keeps the dmgs of
# earlier releases: generate_appcast lists every archive in it in the feed, and makes delta
# updates from the older ones. Release notes go next to a dmg, with its name and .html or
# .md (Flecha-0.2.0.md). The dmgs are published at $FLECHA_DOWNLOAD_URL_PREFIX
# (https://flecha.dev/releases/ by default), and the feed at https://flecha.dev/appcast.xml.
# Nothing is uploaded: publishing the folder is up to you.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
. "$here/scripts/env.sh"

if [ $# -lt 1 ] || [ $# -gt 2 ]; then
  echo "usage: $0 <Flecha-<version>.dmg> [<folder>]" >&2
  exit 1
fi
dmg="$1"
if [ ! -f "$dmg" ]; then
  echo "error: no $dmg" >&2
  exit 1
fi
release="${FLECHA_RELEASE_DIR:-$src/out/Release/flecha-release}"
folder="${2:-$release/appcast}"
prefix="${FLECHA_DOWNLOAD_URL_PREFIX:-https://flecha.dev/releases/}"
sparkle="$("$here/scripts/fetch-sparkle.sh")"

# Sparkle installs what it downloads only if Gatekeeper would open it: a Developer ID
# signed, notarized and stapled dmg.
echo "== check $dmg"
codesign --verify --strict "$dmg"
xcrun stapler validate "$dmg"
spctl --assess --type open --context context:primary-signature "$dmg"

mkdir -p "$folder"
cp -f "$dmg" "$folder/"

echo "== generate_appcast $folder"
"$sparkle/bin/generate_appcast" \
  --download-url-prefix "$prefix" \
  --link "https://flecha.dev" \
  "$folder"

feed="$folder/appcast.xml"
if [ ! -f "$feed" ] || ! grep -q 'sparkle:edSignature' "$feed"; then
  echo "error: $feed is missing or has no EdDSA signature" >&2
  exit 1
fi
echo "== $(basename "$dmg"): $("$sparkle/bin/sign_update" "$folder/$(basename "$dmg")")"
cat <<EOF

Made $feed. To publish, upload the folder's dmgs to $prefix and appcast.xml to
https://flecha.dev/appcast.xml.
EOF
