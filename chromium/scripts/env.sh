# Sourced by the other scripts: where Chromium lives, with the APFS build volume mounted.
#
# The checkout sits on an APFS volume (a disk image on the external disk) because the disk
# itself is HFS+, whose one-second file times make siso think every output is stale, so
# every build was a full two-hour rebuild.
FLECHA_DISK="${FLECHA_DISK:-/Volumes/EXT_FLASH_1TB/projs/flecha}"
FLECHA_VOLUME="${FLECHA_VOLUME:-/Volumes/FlechaBuild}"
if [ ! -d "$FLECHA_VOLUME/chromium" ]; then
  hdiutil attach -quiet "$FLECHA_DISK/FlechaBuild.sparsebundle"
fi
src="${CHROMIUM_SRC:-$FLECHA_VOLUME/chromium/src}"
export PATH="${DEPOT_TOOLS:-$FLECHA_DISK/depot_tools}:$PATH"
