#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
configuration="${1:-release}"
artifacts="$PWD/../artifacts"
version="$(plutil -extract version raw -o - ../shared/version.json)"
build="$(plutil -extract build raw -o - ../shared/version.json)"
cp ../shared/assets/* Sources/QuotaBar/Resources/
export CLANG_MODULE_CACHE_PATH="$PWD/.build/module-cache"
export SWIFTPM_MODULECACHE_OVERRIDE="$PWD/.build/module-cache"
mkdir -p .build/cache .build/config .build/security "$artifacts"
options=(--build-system native --cache-path .build/cache --config-path .build/config --security-path .build/security --disable-sandbox -c "$configuration")
swift build "${options[@]}" --product QuotaBar
binary_dir="$(swift build "${options[@]}" --show-bin-path)"
app="$artifacts/QuotaBar.app"
# Keep the original bundle identifiers for upgrades; all visible names are QuotaBar.
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary_dir/QuotaBar" "$app/Contents/MacOS/QuotaBar"
cp ../LICENSE ../NOTICE "$app/Contents/Resources/"
cp -R "$binary_dir/QuotaBar_QuotaBar.bundle" "$app/Contents/Resources/"
cat > "$app/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>QuotaBar</string>
<key>CFBundleIdentifier</key><string>com.aiusage.lite</string>
<key>CFBundleName</key><string>QuotaBar</string>
<key>CFBundleDisplayName</key><string>QuotaBar</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.2.2</string>
<key>CFBundleVersion</key><string>5</string>
<key>CFBundleIconFile</key><string>AppIcon-4</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
swift scripts/make-icon.swift "$artifacts/AppIcon.iconset" "$PWD/Sources/QuotaBar/Resources/AppMark.svg"
iconutil -c icns "$artifacts/AppIcon.iconset" -o "$app/Contents/Resources/AppIcon-4.icns"
cp "$artifacts/AppIcon.iconset/icon_256x256@2x.png" "$app/Contents/Resources/AppDockIcon.png"
rm -f "$app/Contents/Resources/AppIcon.icns"
if [[ "$configuration" == "release" ]]; then strip -x "$app/Contents/MacOS/QuotaBar"; fi
helper="$app/Contents/Helpers/QuotaBar Dashboard.app"
mkdir -p "$helper/Contents/MacOS" "$helper/Contents/Resources"
cp "$app/Contents/MacOS/QuotaBar" "$helper/Contents/MacOS/QuotaBar"
cp -R "$binary_dir/QuotaBar_QuotaBar.bundle" "$helper/Contents/Resources/"
cp "$app/Contents/Resources/AppIcon-4.icns" "$helper/Contents/Resources/"
cat > "$helper/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>QuotaBar</string>
<key>CFBundleIdentifier</key><string>com.aiusage.lite.dashboard</string>
<key>CFBundleName</key><string>QuotaBar</string>
<key>CFBundleDisplayName</key><string>QuotaBar</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>0.2.2</string>
<key>CFBundleVersion</key><string>5</string>
<key>CFBundleIconFile</key><string>AppIcon-4</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>LSUIElement</key><true/>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
for bundle in "$app" "$helper"; do
  /usr/libexec/PlistBuddy -c "Set :CFBundleShortVersionString $version" "$bundle/Contents/Info.plist"
  /usr/libexec/PlistBuddy -c "Set :CFBundleVersion $build" "$bundle/Contents/Info.plist"
done
codesign --force --sign - --identifier com.aiusage.lite.dashboard "$helper"
codesign --force --sign - --identifier com.aiusage.lite "$app"
codesign --verify --strict "$app"
printf 'App: %s\n' "$app"
du -sh "$app"
