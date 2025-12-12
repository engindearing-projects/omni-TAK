#!/bin/bash
set -e

# OmniTAK macOS DMG Builder
# Builds a distributable .dmg installer for macOS

echo "🔨 Building OmniTAK DMG for macOS..."

# Configuration
APP_NAME="OmniTAK"
VERSION="0.2.0"
BUNDLE_ID="com.omnitak.gui"
DIST_DIR="dist"
APP_BUNDLE="${DIST_DIR}/${APP_NAME}.app"
DMG_NAME="${APP_NAME}-${VERSION}.dmg"
DMG_PATH="${DIST_DIR}/${DMG_NAME}"

# Clean previous builds
echo "🧹 Cleaning previous builds..."
rm -rf "${APP_BUNDLE}"
rm -f "${DMG_PATH}"

# Create dist directory
mkdir -p "${DIST_DIR}"

# Build release binaries
echo "⚙️  Building release binaries..."
cargo build --bin omnitak-gui --release
cargo build --bin omnitak --release

# Create app bundle structure
echo "📦 Creating app bundle structure..."
mkdir -p "${APP_BUNDLE}/Contents/MacOS"
mkdir -p "${APP_BUNDLE}/Contents/Resources"

# Copy binaries
echo "📋 Copying binaries..."
cp target/release/omnitak-gui "${APP_BUNDLE}/Contents/MacOS/${APP_NAME}"
cp target/release/omnitak "${APP_BUNDLE}/Contents/MacOS/omnitak-server"
chmod +x "${APP_BUNDLE}/Contents/MacOS/${APP_NAME}"
chmod +x "${APP_BUNDLE}/Contents/MacOS/omnitak-server"

# Create Info.plist
echo "📝 Creating Info.plist..."
cat > "${APP_BUNDLE}/Contents/Info.plist" << EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleDisplayName</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIdentifier</key>
    <string>${BUNDLE_ID}</string>
    <key>CFBundleVersion</key>
    <string>${VERSION}</string>
    <key>CFBundleShortVersionString</key>
    <string>${VERSION}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleSignature</key>
    <string>????</string>
    <key>CFBundleExecutable</key>
    <string>${APP_NAME}</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon</string>
    <key>LSMinimumSystemVersion</key>
    <string>10.13</string>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSSupportsAutomaticGraphicsSwitching</key>
    <true/>
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <key>NSHumanReadableCopyright</key>
    <string>Copyright 2024 OmniTAK Team</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
</dict>
</plist>
EOF

# Create PkgInfo
echo "APPL????" > "${APP_BUNDLE}/Contents/PkgInfo"

# Create temporary DMG directory
echo "💿 Creating DMG..."
TEMP_DMG_DIR=$(mktemp -d)
cp -R "${APP_BUNDLE}" "${TEMP_DMG_DIR}/"

# Create a symbolic link to /Applications for easy drag-and-drop install
ln -s /Applications "${TEMP_DMG_DIR}/Applications"

# Create DMG using hdiutil
hdiutil create -volname "${APP_NAME}" \
    -srcfolder "${TEMP_DMG_DIR}" \
    -ov -format UDZO \
    "${DMG_PATH}"

# Clean up temp directory
rm -rf "${TEMP_DMG_DIR}"

# Get DMG size
DMG_SIZE=$(du -h "${DMG_PATH}" | cut -f1)

echo ""
echo "✅ DMG created successfully!"
echo "📍 Location: ${DMG_PATH}"
echo "📊 Size: ${DMG_SIZE}"
echo ""
echo "To install:"
echo "  1. Double-click ${DMG_NAME}"
echo "  2. Drag ${APP_NAME}.app to Applications folder"
echo "  3. Launch from Applications or Spotlight"
echo ""
echo "To distribute:"
echo "  - Upload ${DMG_PATH} to your distribution channel"
echo "  - Users can download and install directly"
echo ""
