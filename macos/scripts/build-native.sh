#!/bin/bash
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
configuration="${1:-release}"
options=(--locked --manifest-path "$repo/Cargo.toml" --target-dir "$repo/target")
if [[ "$configuration" == "release" ]]; then options+=(--release); fi
export MACOSX_DEPLOYMENT_TARGET=14.0
cargo build "${options[@]}" --package quotabar-native
cargo build "${options[@]}" --package quotabar --bin quotabar-monitor --bin quotabar-statusline
cargo build "${options[@]}" --package quotabar-ui --bin quotabar-dashboard
