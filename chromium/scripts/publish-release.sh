#!/bin/bash
# Publishes a release made by make-appcast.sh to https://dl.flecha.dev/, the flecha-releases
# R2 bucket on Cloudflare: the dmg under its own name, again as the stable name the site's
# download button links to (Flecha.dmg, or Flecha-x64.dmg with FLECHA_ARCH=x64), any delta
# updates generate_appcast made, and last the feed, so it never lists a dmg not yet there.
#
#   FLECHA_ARCH=arm64|x64 chromium/scripts/publish-release.sh <Flecha-<version>[-x64].dmg> [<folder>]
#
# The folder is make-appcast.sh's ($FLECHA_RELEASE_DIR/appcast by default). Uploads go
# through wrangler (npx, nothing installed), signed in once with: npx wrangler@4 login.
# flecha.dev/appcast.xml and appcast-x64.xml, the SUFeedURLs, are rewritten by the site
# (Vercel) to the same files on dl.flecha.dev.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
. "$here/scripts/env.sh"

if [ $# -lt 1 ] || [ $# -gt 2 ]; then
  echo "usage: $0 <Flecha-<version>[-x64].dmg> [<folder>]" >&2
  exit 1
fi
dmg="$1"
release="${FLECHA_RELEASE_DIR:-$release_out/flecha-release}"
folder="${2:-$release/appcast}"
bucket="${FLECHA_R2_BUCKET:-flecha-releases}"
name="$(basename "$dmg")"
feed="$folder/$release_feed"
if [ ! -f "$folder/$name" ] || [ ! -f "$feed" ]; then
  echo "error: no $folder/$name or $feed; run make-appcast.sh $dmg first" >&2
  exit 1
fi
if ! grep -q "$name" "$feed"; then
  echo "error: $feed does not list $name" >&2
  exit 1
fi

put() { # <file> <key> <content type> <cache control>
  echo "== $bucket/$2"
  npx --yes wrangler@4 r2 object put "$bucket/$2" --remote --file "$1" \
    --content-type "$3" --cache-control "$4"
}

# A versioned dmg never changes; the stable name and the feed change with every release.
put "$folder/$name" "$name" application/x-apple-diskimage "public, max-age=31536000, immutable"
for delta in "$folder"/*.delta; do
  [ -f "$delta" ] || continue
  put "$delta" "$(basename "$delta")" application/octet-stream "public, max-age=31536000, immutable"
done
put "$folder/$name" "Flecha$release_suffix.dmg" application/x-apple-diskimage "public, max-age=300"
put "$feed" "$release_feed" application/xml "public, max-age=300"

cat <<EOF

Published to https://dl.flecha.dev/:
  $name
  Flecha$release_suffix.dmg (the site's download link)
  $release_feed (https://flecha.dev/$release_feed, the app's update feed)
EOF
