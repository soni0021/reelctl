#!/bin/bash
# dist/ReelStudio-<version>.dmg: the app and an Applications link, compressed (UDZO).
set -euo pipefail
: "${VERSION:?}" "${DIST:?}"
APP="$DIST/ReelStudio.app"
[ -d "$APP" ] || { echo "error: $APP is missing; run make app first" >&2; exit 2; }
STAGE="$(mktemp -d "${TMPDIR:-/tmp}/reelstudio-dmg.XXXXXX")"
trap 'rm -rf "$STAGE"' EXIT
cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
DMG="$DIST/ReelStudio-$VERSION.dmg"
rm -f "$DMG"
hdiutil create -volname "ReelStudio $VERSION" -srcfolder "$STAGE" -ov -format UDZO "$DMG" >/dev/null
echo "dmg=$DMG size=$(stat -f%z "$DMG")"
