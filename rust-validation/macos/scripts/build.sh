#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
configuration="${1:-release}"
artifacts="$PWD/../artifacts"
version="$(plutil -extract version raw -o - ../shared/version.json)"
build="$(plutil -extract build raw -o - ../shared/version.json)"
mkdir -p "$artifacts"
bash scripts/build-native.sh "$configuration"
app="$artifacts/QuotaBar.app"
# Keep the original bundle identifiers for upgrades; all visible names are QuotaBar.
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$PWD/../target/$configuration/quotabar-statusline" "$app/Contents/MacOS/quotabar-statusline"
codesign --force --sign - "$app/Contents/MacOS/quotabar-statusline"
cp "$PWD/../target/$configuration/quotabar-monitor" "$app/Contents/MacOS/QuotaBar"
cp ../LICENSE ../NOTICE "$app/Contents/Resources/"
resources="$app/Contents/Resources/QuotaBar_QuotaBar.bundle/Contents"
mkdir -p "$resources/Resources"
cp ../shared/assets/* "$resources/Resources/"
cat > "$resources/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.aiusage.lite.resources</string>
<key>CFBundlePackageType</key><string>BNDL</string>
<key>CFBundleDevelopmentRegion</key><string>en</string>
</dict></plist>
PLIST
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>QuotaBar</string>
<key>CFBundleIdentifier</key><string>com.aiusage.lite</string>
<key>CFBundleName</key><string>QuotaBar</string>
<key>CFBundleDisplayName</key><string>QuotaBar</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>$build</string>
<key>CFBundleIconFile</key><string>AppIcon-4</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>LSApplicationCategoryType</key><string>public.app-category.utilities</string>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
swift scripts/make-icon.swift "$artifacts/AppIcon.iconset" "$PWD/../shared/assets/AppMark.svg"
iconutil -c icns "$artifacts/AppIcon.iconset" -o "$app/Contents/Resources/AppIcon-4.icns"
cp "$artifacts/AppIcon.iconset/icon_256x256@2x.png" "$app/Contents/Resources/AppDockIcon.png"
rm -f "$app/Contents/Resources/AppIcon.icns"
if [[ "$configuration" == "release" ]]; then strip -x "$app/Contents/MacOS/QuotaBar"; fi
helper="$app/Contents/Helpers/QuotaBar Dashboard.app"
mkdir -p "$helper/Contents/MacOS" "$helper/Contents/Resources"
cp "$PWD/../target/$configuration/quotabar-dashboard" "$helper/Contents/MacOS/QuotaBar"
cp ../LICENSE ../NOTICE "$helper/Contents/Resources/"
cp "$app/Contents/Resources/AppIcon-4.icns" "$helper/Contents/Resources/"
cat > "$helper/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>QuotaBar</string>
<key>CFBundleIdentifier</key><string>com.aiusage.lite.dashboard</string>
<key>CFBundleName</key><string>QuotaBar</string>
<key>CFBundleDisplayName</key><string>QuotaBar</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$version</string>
<key>CFBundleVersion</key><string>$build</string>
<key>CFBundleIconFile</key><string>AppIcon-4</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>LSUIElement</key><true/>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
codesign --force --sign - --identifier com.aiusage.lite.dashboard "$helper"
codesign --force --sign - --identifier com.aiusage.lite "$app"
codesign --verify --strict "$app"
printf 'App: %s\n' "$app"
du -sh "$app"
