#!/bin/sh
# Build a release binary and wrap it in target/release/DAW.app.
# Usage: scripts/bundle-app.sh [--install]
#   --install  also copy the bundle to /Applications
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

cargo build --release -p daw

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
app=target/release/DAW.app

rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/daw "$app/Contents/MacOS/daw"

cat > "$app/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>DAW</string>
    <key>CFBundleDisplayName</key>
    <string>DAW</string>
    <key>CFBundleIdentifier</key>
    <string>com.tripplyons.daw</string>
    <key>CFBundleExecutable</key>
    <string>daw</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleVersion</key>
    <string>$version</string>
    <key>CFBundleShortVersionString</key>
    <string>$version</string>
    <key>UTExportedTypeDeclarations</key>
    <array>
        <dict>
            <key>UTTypeIdentifier</key>
            <string>com.tripplyons.daw.project</string>
            <key>UTTypeDescription</key>
            <string>DAW project</string>
            <key>UTTypeConformsTo</key>
            <array>
                <string>public.data</string>
                <string>public.content</string>
            </array>
            <key>UTTypeTagSpecification</key>
            <dict>
                <key>public.filename-extension</key>
                <array>
                    <string>dawproj</string>
                </array>
            </dict>
        </dict>
    </array>
    <key>CFBundleDocumentTypes</key>
    <array>
        <dict>
            <key>CFBundleTypeName</key>
            <string>DAW project</string>
            <key>CFBundleTypeRole</key>
            <string>Editor</string>
            <key>LSHandlerRank</key>
            <string>Owner</string>
            <key>LSItemContentTypes</key>
            <array>
                <string>com.tripplyons.daw.project</string>
            </array>
        </dict>
    </array>
    <key>LSMinimumSystemVersion</key>
    <string>11.0</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
EOF

# Ad-hoc signature so Gatekeeper runs the local build; not for distribution.
codesign --force --sign - "$app"

# Tell Launch Services about the bundle so Finder opens .dawproj files with it.
lsregister=/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister

if [ "${1:-}" = "--install" ]; then
    installed=/Applications/DAW.app
    if [ -e "$installed" ]; then
        id=$(defaults read "$installed/Contents/Info.plist" CFBundleIdentifier 2>/dev/null || true)
        if [ "$id" != com.tripplyons.daw ]; then
            echo "$installed is a different app ($id); not replacing it" >&2
            exit 1
        fi
    fi
    rm -rf "$installed"
    cp -R "$app" "$installed"
    "$lsregister" -f "$installed"
    # Only the installed copy should handle .dawproj files.
    "$lsregister" -u "$root/$app"
    echo "installed $installed"
else
    "$lsregister" -f "$app"
    echo "built $app"
fi
