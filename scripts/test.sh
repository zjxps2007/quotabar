#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
export CLANG_MODULE_CACHE_PATH="$PWD/.build/module-cache"
export SWIFTPM_MODULECACHE_OVERRIDE="$PWD/.build/module-cache"
mkdir -p artifacts
developer_dir="$(xcode-select -p)"
frameworks="$developer_dir/Library/Developer/Frameworks"
if [[ ! -d "$frameworks/Testing.framework" ]]; then
  frameworks="$developer_dir/Platforms/MacOSX.platform/Developer/Library/Frameworks"
fi
swift test --build-system native --cache-path .build/cache --config-path .build/config --security-path .build/security --disable-sandbox \
  --disable-xctest --enable-swift-testing \
  -Xswiftc -F -Xswiftc "$frameworks" \
  -Xlinker -F -Xlinker "$frameworks" -Xlinker -rpath -Xlinker "$frameworks" \
  "$@" 2>&1 | tee artifacts/tests.log
