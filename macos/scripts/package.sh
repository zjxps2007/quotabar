#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../.."
repo="$PWD"
bash macos/scripts/build.sh release
version="$(plutil -extract version raw -o - shared/version.json)"
build="$(plutil -extract build raw -o - shared/version.json)"
app="$repo/artifacts/QuotaBar.app"
helper="$app/Contents/Helpers/QuotaBar Dashboard.app"
architecture="$(lipo -archs "$app/Contents/MacOS/QuotaBar")"
case "$architecture" in arm64|x86_64) ;; *) echo "Unsupported package architecture: $architecture" >&2; exit 1 ;; esac
for bundle in "$app" "$helper"; do
  test "$(plutil -extract CFBundleShortVersionString raw -o - "$bundle/Contents/Info.plist")" = "$version"
  test "$(plutil -extract CFBundleVersion raw -o - "$bundle/Contents/Info.plist")" = "$build"
  cmp "$repo/LICENSE" "$bundle/Contents/Resources/LICENSE"
  cmp "$repo/NOTICE" "$bundle/Contents/Resources/NOTICE"
  lipo "$bundle/Contents/MacOS/QuotaBar" -verify_arch "$architecture"
  codesign --verify --strict "$bundle"
done
archive="$repo/artifacts/QuotaBar-$version-macOS-$architecture.zip"
stage="$(mktemp -d "$repo/artifacts/macos-package.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
ditto -c -k --sequesterRsrc --keepParent "$app" "$stage/package.zip"
unzip -tq "$stage/package.zip"
mv "$stage/package.zip" "$archive"
python3 - "$repo" "$archive" "$architecture" <<'PY'
import hashlib
import json
import pathlib
import subprocess
import sys

repo, archive = map(pathlib.Path, sys.argv[1:3])
version = json.loads((repo / 'shared/version.json').read_text())
metadata = {
    'schema_version': 1,
    **version,
    'application_commit': subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'HEAD'], text=True).strip(),
    'source_dirty': bool(subprocess.check_output(['git', '-C', str(repo), 'status', '--porcelain'], text=True).strip()),
    'architecture': sys.argv[3],
    'minimum_macos': '14.0',
    'code_signing': 'ad-hoc',
    'notarized': False,
    'application_zip': {
        'name': archive.name,
        'bytes': archive.stat().st_size,
        'sha256': hashlib.sha256(archive.read_bytes()).hexdigest(),
    },
}
(repo / 'artifacts/macos-build-info.json').write_text(json.dumps(metadata, indent=2) + '\n')
print(f'Package: {archive}')
print(f'SHA256: {metadata["application_zip"]["sha256"]}')
PY
