#!/bin/bash
set -e

echo "Building release binary..."
cargo build --release

echo "Creating CMDCTL.app bundle..."
APP_DIR="bundle/CMDCTL.app"
mkdir -p "$APP_DIR/Contents/MacOS"
mkdir -p "$APP_DIR/Contents/Resources"

cp target/release/cmdctl "$APP_DIR/Contents/MacOS/cmdctl"

echo "Bundle created at $APP_DIR"
echo ""
echo "To install: cp -r bundle/CMDCTL.app /Applications/"
echo "To run:     open bundle/CMDCTL.app"
