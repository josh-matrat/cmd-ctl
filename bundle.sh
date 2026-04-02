#!/bin/bash
set -e

APP_NAME="CMDCTL"
APP_DIR="bundle/${APP_NAME}.app"
DMG_NAME="${APP_NAME}.dmg"
DMG_STAGING="bundle/dmg-staging"

echo "Building release binary..."
cargo build --release

echo "Creating ${APP_NAME}.app bundle..."
rm -rf "$APP_DIR"
mkdir -p "$APP_DIR/Contents/MacOS"
mkdir -p "$APP_DIR/Contents/Resources"

cp target/release/cmdctl "$APP_DIR/Contents/MacOS/cmdctl"
cp bundle/Info.plist "$APP_DIR/Contents/Info.plist" 2>/dev/null || true

# App icon — place a 1024x1024 PNG at bundle/icon_1024.png, then run this script
ICON_SRC="bundle/icon_1024.png"
if [ -f "$ICON_SRC" ]; then
    echo "Generating AppIcon.icns..."
    ICONSET="bundle/AppIcon.iconset"
    rm -rf "$ICONSET"
    mkdir -p "$ICONSET"
    sips -z 16 16     "$ICON_SRC" --out "$ICONSET/icon_16x16.png"      > /dev/null
    sips -z 32 32     "$ICON_SRC" --out "$ICONSET/icon_16x16@2x.png"   > /dev/null
    sips -z 32 32     "$ICON_SRC" --out "$ICONSET/icon_32x32.png"      > /dev/null
    sips -z 64 64     "$ICON_SRC" --out "$ICONSET/icon_32x32@2x.png"   > /dev/null
    sips -z 128 128   "$ICON_SRC" --out "$ICONSET/icon_128x128.png"    > /dev/null
    sips -z 256 256   "$ICON_SRC" --out "$ICONSET/icon_128x128@2x.png" > /dev/null
    sips -z 256 256   "$ICON_SRC" --out "$ICONSET/icon_256x256.png"    > /dev/null
    sips -z 512 512   "$ICON_SRC" --out "$ICONSET/icon_256x256@2x.png" > /dev/null
    sips -z 512 512   "$ICON_SRC" --out "$ICONSET/icon_512x512.png"    > /dev/null
    sips -z 1024 1024 "$ICON_SRC" --out "$ICONSET/icon_512x512@2x.png" > /dev/null
    iconutil -c icns "$ICONSET" -o "$APP_DIR/Contents/Resources/AppIcon.icns"
    rm -rf "$ICONSET"
else
    echo "Warning: No icon found at $ICON_SRC — app will use default macOS icon"
fi

echo "Creating ${DMG_NAME}..."
rm -rf "$DMG_STAGING" "$DMG_NAME"
mkdir -p "$DMG_STAGING"
cp -r "$APP_DIR" "$DMG_STAGING/"
ln -s /Applications "$DMG_STAGING/Applications"

hdiutil create \
    -volname "$APP_NAME" \
    -srcfolder "$DMG_STAGING" \
    -ov \
    -format UDZO \
    "$DMG_NAME"

rm -rf "$DMG_STAGING"

# Install to /Applications so Spotlight always has the latest build
echo "Installing to /Applications..."
rm -rf "/Applications/${APP_NAME}.app"
cp -r "$APP_DIR" "/Applications/${APP_NAME}.app"

echo ""
echo "Done! Created:"
echo "  App:  $APP_DIR"
echo "  DMG:  $DMG_NAME"
echo "  Installed to /Applications/${APP_NAME}.app"
echo ""
echo "Email $DMG_NAME to your team."
echo "Recipients: double-click the DMG, drag CMDCTL to Applications."
echo "Note: First launch requires right-click → Open (unsigned app)."
