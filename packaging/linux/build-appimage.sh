#!/bin/bash
# Makes TuplePad's Linux downloads from a release build (on the CI's Ubuntu machine):
#   dist/TuplePad-<version>-linux-x64.AppImage  (one file that runs on most distributions)
#   dist/TuplePad-<version>-linux-x64.tar.gz    (the app, the tuplepad command, icon and desktop entry)
#
#   cargo build --release --manifest-path core/Cargo.toml -p tuplepad-app -p tuplepad-cli
#   packaging/linux/build-appimage.sh v001
#
# linuxdeploy gathers the libraries TuplePad needs into the AppImage, except the desktop's own
# (GTK, GLib, Cairo, Pango and the like), which every desktop already has.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
VERSION="${1:?usage: build-appimage.sh <version>}"
BIN="$ROOT/core/target/release"
DIST="$ROOT/dist"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$DIST"

APPDIR="$WORK/AppDir"
mkdir -p "$APPDIR/usr/bin"
cp "$BIN/tuplepad-app" "$BIN/tuplepad" "$APPDIR/usr/bin/"
cp "$ROOT/packaging/linux/tuplepad.png" "$WORK/tuplepad.png"

curl -fsSL -o "$WORK/linuxdeploy" https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-x86_64.AppImage
chmod +x "$WORK/linuxdeploy"
export APPIMAGE_EXTRACT_AND_RUN=1
export LDAI_OUTPUT="$DIST/TuplePad-$VERSION-linux-x64.AppImage"
EXCLUDES=()
for LIBRARY in 'libgtk-3.so*' 'libgdk-3.so*' 'libgdk_pixbuf-2.0.so*' 'libpango*' 'libcairo*' 'libatk*' \
               'libglib-2.0.so*' 'libgobject-2.0.so*' 'libgio-2.0.so*' 'libgmodule-2.0.so*' 'libharfbuzz*' \
               'libepoxy*' 'libwayland-*' 'libxkbcommon*' 'libdbus-1.so*'; do
    EXCLUDES+=(--exclude-library "$LIBRARY")
done
"$WORK/linuxdeploy" --appdir "$APPDIR" \
    --executable "$APPDIR/usr/bin/tuplepad-app" \
    --desktop-file "$ROOT/packaging/linux/tuplepad.desktop" \
    --icon-file "$WORK/tuplepad.png" \
    "${EXCLUDES[@]}" \
    --output appimage

PLAIN="$WORK/TuplePad-$VERSION"
mkdir -p "$PLAIN"
cp "$BIN/tuplepad-app" "$BIN/tuplepad" "$ROOT/packaging/linux/tuplepad.desktop" "$ROOT/packaging/linux/tuplepad.png" "$ROOT/LICENSE" "$PLAIN/"
tar -C "$WORK" -czf "$DIST/TuplePad-$VERSION-linux-x64.tar.gz" "TuplePad-$VERSION"
ls -l "$DIST"
