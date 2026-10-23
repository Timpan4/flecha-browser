#!/bin/bash
# Publishes a release made by make-appcast.sh to https://dl.flecha.dev/, the flecha-releases
# R2 bucket on Cloudflare, which holds only the newest release of each CPU: the dmg under
# its own name (the feed's link, which never changes under a cached feed), again as the
# stable name the site's download button links to (Flecha.dmg, or Flecha-x64.dmg with
# FLECHA_ARCH=x64), the delta updates the feed lists, and last the feed itself, so it never
# lists a file not yet there. Then it deletes what the previous feed listed and the new one
# doesn't: the older release's dmg and deltas. Older releases stay on GitHub.
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
prefix="${FLECHA_DOWNLOAD_URL_PREFIX:-https://dl.flecha.dev/}"
name="$(basename "$dmg")"
stable="Flecha$release_suffix.dmg"
feed="$folder/$release_feed"
if [ ! -f "$folder/$name" ] || [ ! -f "$feed" ]; then
  echo "error: no $folder/$name or $feed; run make-appcast.sh $dmg first" >&2
  exit 1
fi

# The files a feed links to on dl.flecha.dev, one per line: the dmg and its deltas.
listed() {
  grep -o "url=\"$prefix[^\"]*\"" | sed -e "s|^url=\"$prefix||" -e 's|"$||' | sort -u
}
new="$(listed <"$feed")"
if ! grep -qx "$name" <<<"$new"; then
  echo "error: $feed does not link to $prefix$name" >&2
  exit 1
fi
old="$(curl --fail --silent --location "$prefix$release_feed" | listed || true)"

r2() {
  npx --yes wrangler@4 r2 object "$@" --remote
}
put() { # <file> <key> <content type> <cache control>
  echo "== put $bucket/$2"
  r2 put "$bucket/$2" --file "$1" --content-type "$3" --cache-control "$4"
}

# A versioned file never changes; the stable name and the feed change with every release.
while read -r key; do
  case "$key" in
    *.dmg) type=application/x-apple-diskimage ;;
    *) type=application/octet-stream ;;
  esac
  put "$folder/$key" "$key" "$type" "public, max-age=31536000, immutable"
done <<<"$new"
put "$folder/$name" "$stable" application/x-apple-diskimage "public, max-age=300"
put "$feed" "$release_feed" application/xml "public, max-age=300"

# Only what the previous feed listed is deleted, never the stable name or the feed.
while read -r key; do
  [ -n "$key" ] || continue
  grep -qx "$key" <<<"$new" && continue
  case "$key" in "$stable" | "$release_feed") continue ;; esac
  echo "== delete $bucket/$key"
  r2 delete "$bucket/$key"
done <<<"$old"

cat <<EOF

Published to $prefix:
  $name
  $stable (the site's download link)
  $release_feed (https://flecha.dev/$release_feed, the app's update feed)
EOF
