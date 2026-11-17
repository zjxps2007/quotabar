#!/usr/bin/env python3
"""Run the archived SwiftUI comparison suite against the current Rust sources."""
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[2]
raise SystemExit(subprocess.call([
    sys.executable, str(root / "scripts/prepare_legacy_validation.py"),
    "--script", "macos/scripts/compare-dashboard.py", "--", *sys.argv[1:],
], cwd=root))
