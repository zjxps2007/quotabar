#!/bin/bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
exec python3 "$repo/scripts/prepare_legacy_validation.py" --script macos/scripts/benchmark-local-usage.sh -- "$@"
