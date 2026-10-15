#!/bin/bash
# Builds TuplePad (the Slint app) and wraps it in a Mac app bundle: build/TuplePad.app.
#
#   scripts/bundle-mac.sh [release|launch|debug]
#
# `launch` (what the launcher uses) is optimized like `release` but rebuilds much faster.
# Cargo's progress shows on stderr; the bundle's path is the only thing on stdout.
#
# The icon is compiled from TuplePad/AppIcon.icon with actool (as Xcode would), and the
# document types come from Config/Info.plist. Signing is ad hoc until the Developer ID is ready.
set -euo pipefail

export PATH="$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PROFILE="${1:-release}"
# Rust's default deployment target for Apple silicon. (Not exported: changing it would make
# Cargo rebuild every C dependency.)
MINIMUM_MACOS=11.0

CARGO_PROFILE="$PROFILE"
[ "$PROFILE" = "debug" ] && CARGO_PROFILE=dev
cargo build --profile "$CARGO_PROFILE" --manifest-path "$ROOT/core/Cargo.toml" -p tuplepad-app -p tuplepad-cli 1>&2
BINARY="$ROOT/core/target/$PROFILE/tuplepad-app"
TOOL="$ROOT/core/target/$PROFILE/tuplepad"

APP="$ROOT/build/TuplePad.app"
STAGE="$ROOT/build/TuplePad.app.tmp"
rm -rf "$STAGE"
mkdir -p "$STAGE/Contents/MacOS" "$STAGE/Contents/Resources" "$STAGE/Contents/Helpers"
cp "$BINARY" "$STAGE/Contents/MacOS/TuplePad"
# The `tuplepad` terminal command (File ▸ Install Command Line Tool… links it onto the PATH).
cp "$TOOL" "$STAGE/Contents/Helpers/tuplepad"

# The icon: Assets.car (with the dark and tinted looks) plus AppIcon.icns for older systems.
PARTIAL="$ROOT/build/icon-partial.plist"
if ! xcrun actool "$ROOT/TuplePad/AppIcon.icon" --compile "$STAGE/Contents/Resources" \
        --platform macosx --minimum-deployment-target "$MINIMUM_MACOS" \
        --app-icon AppIcon --output-partial-info-plist "$PARTIAL" >/dev/null; then
    echo "warning: couldn't compile the app icon" >&2
fi

# Versions are named v001, v002, … (VERSION at the repository's root); the build number is
# the plain number, which Apple wants numeric.
VERSION="$(tr -d '[:space:]' < "$ROOT/VERSION")"
BUILD_NUMBER="$((10#${VERSION#v}))"
PLIST="$STAGE/Contents/Info.plist"
cp "$ROOT/Config/Info.plist" "$PLIST"
set_key() { /usr/libexec/PlistBuddy -c "Add :$1 $2 $3" "$PLIST" 2>/dev/null || /usr/libexec/PlistBuddy -c "Set :$1 $3" "$PLIST"; }
set_key CFBundleIdentifier string com.bazstudios.TuplePad
set_key CFBundleName string TuplePad
set_key CFBundleDisplayName string TuplePad
set_key CFBundleExecutable string TuplePad
set_key CFBundlePackageType string APPL
set_key CFBundleShortVersionString string "$VERSION"
set_key CFBundleVersion string "$BUILD_NUMBER"
set_key CFBundleIconFile string AppIcon
set_key CFBundleIconName string AppIcon
set_key LSMinimumSystemVersion string "$MINIMUM_MACOS"
set_key LSApplicationCategoryType string public.app-category.developer-tools
set_key NSHighResolutionCapable bool true
set_key NSSupportsAutomaticGraphicsSwitching bool true
set_key NSHumanReadableCopyright string "MIT License"

codesign --force --sign - "$STAGE/Contents/Helpers/tuplepad" >/dev/null 2>&1
codesign --force --sign - "$STAGE" >/dev/null 2>&1
rm -rf "$APP"
mv "$STAGE" "$APP"
echo "$APP"
