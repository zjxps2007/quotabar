#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/../.."
mkdir -p artifacts
cargo test --locked --workspace --all-targets "$@" 2>&1 | tee artifacts/tests.log
