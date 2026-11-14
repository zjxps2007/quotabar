#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p ../artifacts .build/module-cache
bash scripts/build-native.sh release
swiftc -O -whole-module-optimization -parse-as-library -module-cache-path .build/module-cache \
  -I Sources/CQuotaBar -Xlinker "$PWD/../target/release/libquotabar_native.a" -liconv \
  Sources/UsageCore/Account.swift Sources/UsageCore/LocalUsage.swift Sources/UsageCore/LocalUsageScanner.swift \
  Sources/UsageCore/Upstream/ProviderProtocol.swift Sources/UsageCore/Upstream/CodexAccountIdentity.swift \
  Sources/UsageCore/Upstream/DateFormatting.swift \
  scripts/benchmark-local-usage.swift -o ../artifacts/local-usage-benchmark
../artifacts/local-usage-benchmark
