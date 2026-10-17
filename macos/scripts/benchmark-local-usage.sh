#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p ../artifacts .build/module-cache
swiftc -O -whole-module-optimization -parse-as-library -module-cache-path .build/module-cache \
  Sources/UsageCore/Account.swift Sources/UsageCore/LocalUsage.swift \
  Sources/UsageCore/Upstream/ProviderProtocol.swift Sources/UsageCore/Upstream/CodexAccountIdentity.swift \
  Sources/UsageCore/Upstream/DateFormatting.swift Sources/UsageCore/Upstream/SessionProjectionReader.swift \
  scripts/benchmark-local-usage.swift -o ../artifacts/local-usage-benchmark
../artifacts/local-usage-benchmark
