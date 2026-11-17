#!/bin/bash
set -euo pipefail
exec bash "$(dirname "$0")/../macos/scripts/benchmark-local-usage.sh" "$@"
