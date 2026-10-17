#!/bin/bash
set -euo pipefail
exec bash "$(dirname "$0")/../macos/scripts/test.sh" "$@"
