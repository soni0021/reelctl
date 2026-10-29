#!/bin/bash
# Assemble dist/ReelStudio.app: the SwiftPM binary, reelctl beside it in
# Contents/MacOS, Info.plist with the version, and an ad-hoc signature
# (inner binary first). Not notarized (docs/specs/mac-app.md).
set -euo pipefail
: "${VERSION:?}" "${ROOT:?}" "${DIST:?}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
APP="$DIST/ReelStudio.app"
BIN="$(cd "$HERE" && swift build -c release --arch arm64 --show-bin-path)"

rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$BIN/ReelStudio" "$APP/Contents/MacOS/ReelStudio"
cp "$ROOT/target/release/reelctl" "$APP/Contents/MacOS/reelctl"
sed "s/__VERSION__/$VERSION/g" "$HERE/Resources/Info.plist" > "$APP/Contents/Info.plist"

codesign --force --sign - --timestamp=none "$APP/Contents/MacOS/reelctl"
codesign --force --sign - --timestamp=none "$APP"
codesign --verify --strict "$APP"
echo "app=$APP version=$VERSION"
