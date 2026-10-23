#!/bin/bash
# Packages the release build (out/Release/Flecha.app, from build-release.sh) into a signed
# app you can copy to /Applications, and with --dmg (or FLECHA_DMG=1) a disk image too:
#
#   chromium/scripts/package-release.sh [--dmg]
#
# Into $FLECHA_RELEASE_DIR (out/Release/flecha-release by default, on the build volume) it
#   1. copies the built app to <release>/Flecha/Flecha.app (rsync, so a re-run starts clean);
#   2. checks the shell the build put in the framework's Resources/flecha-shell (patch 0050)
#      against chromium/shell/dist, and copies dist over it if they differ (the GN copy only
#      notices a changed staging folder, not a changed file inside it);
#   3. adds the native messaging host: Node as Contents/Helpers/flecha-node, the bundled host
#      script and a launcher in Contents/Resources/native-host. Chromium needs an absolute
#      manifest path, so register it once the app is in place:
#      chromium/scripts/install-native-host.sh --app /Applications/Flecha.app
#   4. signs everything, innermost first: ad hoc (codesign --sign -) unless
#      FLECHA_SIGN_IDENTITY names a keychain identity ("Developer ID Application: …"), which
#      also gets the hardened runtime, a timestamp and Chromium's entitlements (as
#      chrome/installer/mac/signing/parts.py does);
#   5. with --dmg, makes <release>/Flecha-<version>.dmg with hdiutil.
# Everything is redone on each run, so it is safe to run again.
#
# Node is $FLECHA_NODE or the node on PATH: 22.5+ (node:sqlite), arm64, and linked only to
# system libraries (an official or nvm build; Homebrew's links its own dylibs). Its LICENSE
# goes along with it.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
. "$here/scripts/env.sh"

dmg="${FLECHA_DMG:-0}"
for arg in "$@"; do
  case "$arg" in
    --dmg) dmg=1 ;;
    *) echo "error: unknown argument $arg" >&2; exit 1 ;;
  esac
done

product="$(sed -n 's/^PRODUCT_FULLNAME=//p' "$here/branding/BRANDING")"
built="${FLECHA_APP_IN:-$src/out/Release/$product.app}"
release="${FLECHA_RELEASE_DIR:-$src/out/Release/flecha-release}"
stage="$release/$product"
app="$stage/$product.app"
fw="$app/Contents/Frameworks/$product Framework.framework"
identity="${FLECHA_SIGN_IDENTITY:--}"
work="$release/work"

if [ ! -d "$built" ]; then
  echo "error: no $built; build it first (chromium/scripts/build-release.sh)" >&2
  exit 1
fi
mkdir -p "$app" "$work"

echo "== copy $built -> $app"
rsync -a --delete "$built/" "$app/"

echo "== shell"
shell_dist="$here/shell/dist"
bundled_shell="$fw/Versions/Current/Resources/flecha-shell"
if [ ! -f "$shell_dist/manifest.json" ]; then
  echo "error: no built shell in $shell_dist (node chromium/shell/build.mjs)" >&2
  exit 1
fi
if ! diff -rq "$shell_dist" "$bundled_shell" >/dev/null 2>&1; then
  echo "note: the app's flecha-shell differs from $shell_dist; copying dist in"
  mkdir -p "$bundled_shell"
  rsync -a --delete "$shell_dist/" "$bundled_shell/"
fi

echo "== native host"
node="${FLECHA_NODE:-$(command -v node || true)}"
if [ -z "$node" ]; then
  echo "error: no node on PATH; set FLECHA_NODE to a Node 22.5+ binary" >&2
  exit 1
fi
node="$(cd "$(dirname "$node")" && pwd -P)/$(basename "$node")"
if ! "$node" -e 'require("node:sqlite")' 2>/dev/null; then
  echo "error: $node has no node:sqlite; the host needs Node 22.5 or newer" >&2
  exit 1
fi
if ! lipo -archs "$node" | grep -qw arm64; then
  echo "error: $node is not an arm64 binary" >&2
  exit 1
fi
if otool -L "$node" | tail -n +2 | awk '{print $1}' | grep -Eqv '^(/usr/lib/|/System/)'; then
  echo "error: $node links libraries outside the system; use an official or nvm Node" >&2
  exit 1
fi
(cd "$here/.." && "$node" chromium/native-host/build.mjs)
mkdir -p "$app/Contents/Helpers" "$app/Contents/Resources/native-host"
cp "$node" "$app/Contents/Helpers/flecha-node"
chmod 755 "$app/Contents/Helpers/flecha-node"
host="$app/Contents/Resources/native-host"
cp "$here/native-host/dist/flecha-host.cjs" "$host/flecha-host.cjs"
cat >"$host/flecha-host" <<'EOF'
#!/bin/sh
# Flecha's native messaging host: the app's own Node runs the bundled host script.
here="$(cd "$(dirname "$0")" && pwd)"
exec "$here/../../Helpers/flecha-node" "$here/flecha-host.cjs" "$@"
EOF
chmod 755 "$host/flecha-host"
node_license=""
for f in "$(dirname "$node")/../LICENSE" "$(dirname "$node")/../share/doc/node/LICENSE"; do
  if [ -f "$f" ]; then node_license="$f"; break; fi
done
if [ -n "$node_license" ]; then
  cp "$node_license" "$host/NODE-LICENSE"
else
  echo "warning: found no LICENSE next to $node; add Node's licence to $host by hand" >&2
fi

echo "== sign ($identity)"
full_options="restrict,library,kill,runtime"
# Library validation is incompatible with the JIT entitlement (parts.py).
jit_options="restrict,kill,runtime"
entitlements="$src/chrome/app"
sign() { # sign <path> [<options> [<entitlements>]]
  local args=(--force --sign "$identity")
  # Ad hoc code has no team, so library validation (implied by the hardened runtime) would
  # stop the app loading its own framework: ad hoc signing seals the bundle and no more.
  if [ "$identity" != "-" ]; then
    args+=(--timestamp)
    if [ -n "${2:-}" ]; then args+=(--options "$2"); fi
    if [ -n "${3:-}" ]; then args+=(--entitlements "$3"); fi
  fi
  codesign "${args[@]}" "$1"
}

# Loose code in the framework (dylibs, crashpad handler, app_mode_loader, ...), then its
# helper apps, then the framework itself.
while IFS= read -r -d '' f; do
  if file -b "$f" | grep -q 'Mach-O'; then sign "$f" "$full_options"; fi
done < <(find -H "$fw/Versions/Current" -name '*.app' -prune -o -type f -print0)
while IFS= read -r -d '' helper; do
  case "$(basename "$helper")" in
    *"(Renderer).app" | *"(Aperitif Renderer).app")
      sign "$helper" "$jit_options" "$entitlements/helper-renderer-entitlements.plist" ;;
    *"(GPU).app" | *"(Aperitif GPU).app")
      sign "$helper" "$jit_options" "$entitlements/helper-gpu-entitlements.plist" ;;
    *) sign "$helper" "$full_options" ;;
  esac
done < <(find -H "$fw/Versions/Current/Helpers" -maxdepth 1 -name '*.app' -print0)
sign "$fw"

# Node keeps the entitlements it shipped with (JIT and the like), less get-task-allow.
node_entitlements="$work/node-entitlements.plist"
codesign -d --entitlements - --xml "$node" >"$node_entitlements" 2>/dev/null || true
if [ ! -s "$node_entitlements" ]; then
  cat >"$node_entitlements" <<'EOF'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>com.apple.security.cs.allow-jit</key>
	<true/>
	<key>com.apple.security.cs.allow-unsigned-executable-memory</key>
	<true/>
	<key>com.apple.security.cs.disable-library-validation</key>
	<true/>
</dict>
</plist>
EOF
fi
/usr/libexec/PlistBuddy -c 'Delete :com.apple.security.get-task-allow' "$node_entitlements" \
  >/dev/null 2>&1 || true
sign "$app/Contents/Helpers/flecha-node" "runtime" "$node_entitlements"

sign "$app" "$full_options" "$entitlements/app-entitlements.plist"
codesign --verify --deep --strict --verbose=1 "$app"
echo "signed $app"

if [ "$dmg" = 1 ]; then
  version="$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$app/Contents/Info.plist")"
  image="$release/$product-$version.dmg"
  echo "== dmg $image"
  ln -sfn /Applications "$stage/Applications"
  hdiutil create -quiet -volname "$product" -srcfolder "$stage" -ov -format UDZO "$image"
  if [ "$identity" != "-" ]; then codesign --force --sign "$identity" --timestamp "$image"; fi
  echo "made $image"
fi

cat <<EOF

Done. To install: copy $app to /Applications, then register its native host:
  chromium/scripts/install-native-host.sh --app /Applications/$product.app
EOF
