#!/usr/bin/env python3
"""Pass clean Rust packages to the archived compatibility suite."""

import argparse
import json
from pathlib import Path
import shutil
import subprocess

from prepare_legacy_validation import ROOT, prepare


def verify_clean_source():
    if subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT).strip():
        raise ValueError("Product packaging must use a clean source checkout")
    return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("platform", choices=["macos", "windows", "verify-windows"])
    options = parser.parse_args()
    commit = verify_clean_source()
    artifacts = ROOT / "artifacts"
    if options.platform == "verify-windows":
        data = json.loads((artifacts / "windows/installer-build-info.json").read_bytes())
        if (data["application_commit"] != commit or data["installer_commit"] != commit
                or data["installer_source_dirty"]):
            raise ValueError("Windows package source metadata does not match the clean checkout")
        print("Windows package source commit and clean status verified")
        return
    destination = prepare() / "artifacts"
    destination.mkdir(exist_ok=True)
    if options.platform == "macos":
        data = json.loads((artifacts / "macos-build-info.json").read_bytes())
        if data["application_commit"] != commit or data["source_dirty"]:
            raise ValueError("Mac package source metadata does not match the clean checkout")
        names = ["QuotaBar.app", "macos-dashboard"]
    else:
        names = ["QuotaBar-Windows-x64"]
    for name in names:
        source = artifacts / name
        if not source.is_dir():
            raise ValueError(f"Missing packaged application or native test report: {source}")
        shutil.copytree(source, destination / name, dirs_exist_ok=True, symlinks=True)
    print(f"Staged current {options.platform} package for legacy compatibility tests")


if __name__ == "__main__":
    main()
