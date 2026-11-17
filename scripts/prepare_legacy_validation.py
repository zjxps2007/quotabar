#!/usr/bin/env python3
"""Prepare an isolated legacy test suite with this checkout's Rust application."""

import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile


ROOT = Path(__file__).resolve().parents[1]
DIRECTORIES = ("native/", "vendor/", "shared/", "windows/assets/", "windows/installer/", "macos/assets/")
FILES = {
    "Cargo.toml", "Cargo.lock", ".cargo/config.toml", "build.rs", "LICENSE", "NOTICE",
    "macos/scripts/build.sh", "macos/scripts/build-native.sh", "macos/scripts/package.sh",
    "windows/scripts/build-native.ps1", "windows/scripts/package.ps1",
    "windows/scripts/package-installer.ps1", "windows/scripts/write-update-version.ps1",
    "windows/scripts/ComponentVersion.csproj", "windows/scripts/get-inno.ps1",
    "windows/scripts/common.ps1", "windows/global.json",
}


def git(*args, cwd=ROOT):
    return subprocess.check_output(["git", *args], cwd=cwd)


def production_files():
    names = git("ls-files", "--cached", "--others", "--exclude-standard", "-z").decode().split("\0")
    return sorted({name for name in names if name and
                   (name in FILES or name.startswith(DIRECTORIES)) and (ROOT / name).is_file()})


def inside(root, path):
    path = path.resolve()
    if not path.is_relative_to(root.resolve()):
        raise ValueError(f"Validation path escaped its workspace: {path}")
    return path


def extract_snapshot(data, destination):
    # Git archives contain only regular files, directories and repository symlinks.
    # Materialize the resource symlink on Windows, where symlink creation may be disabled.
    links = []
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        for member in archive:
            path = inside(destination, destination / member.name)
            if member.isdir():
                path.mkdir(parents=True, exist_ok=True)
            elif member.issym():
                links.append((destination / member.name, member.linkname))
            elif member.isfile():
                path.parent.mkdir(parents=True, exist_ok=True)
                with archive.extractfile(member) as source:
                    path.write_bytes(source.read())
                path.chmod(member.mode)
            else:
                raise ValueError(f"Unexpected legacy archive entry: {member.name}")
    for path, name in links:
        target = inside(destination, path.parent / name)
        inside(destination, path.parent)
        path.parent.mkdir(parents=True, exist_ok=True)
        if path.is_symlink() or path.is_file():
            path.unlink()
        if path.exists():
            raise ValueError(f"Unexpected resource directory: {path}")
        if os.name == "nt":
            shutil.copytree(target, path) if target.is_dir() else shutil.copy2(target, path)
        else:
            path.symlink_to(name, target_is_directory=target.is_dir())


def prepare(destination=None):
    metadata = json.loads((ROOT / "shared/legacy-validation.json").read_bytes())
    revision = git("rev-parse", "HEAD").decode().strip()
    files = production_files()
    digest = hashlib.sha256()
    hashes = {}
    for name in files:
        value = hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
        hashes[name] = value
        digest.update((name + "\0" + value + "\n").encode())
    fingerprint = digest.hexdigest()
    marker = {"sourceRepository": str(ROOT), "sourceCommit": revision,
              "sourceFingerprint": fingerprint, "legacyCommit": metadata["legacyCommit"]}
    if destination is None:
        destination = ROOT / "target/legacy-validation" / (revision[:7] + "-" + fingerprint[:12])
    destination = destination.resolve()
    if destination == ROOT or ROOT.is_relative_to(destination):
        raise ValueError("The validation workspace must not contain the source checkout")
    receipt = destination / ".quotabar-legacy-validation.json"
    if destination.exists():
        if not receipt.is_file() or json.loads(receipt.read_bytes()) != marker:
            raise ValueError(f"Destination is not this source's prepared workspace: {destination}")
        if any(not (destination / name).is_file() or
               hashlib.sha256((destination / name).read_bytes()).hexdigest() != value
               for name, value in hashes.items()):
            raise ValueError("Production files in the validation workspace were modified")
        return destination
    archive = metadata["legacyCommit"] + ":" + metadata["archiveDirectory"]
    if subprocess.run(["git", "cat-file", "-e", archive], cwd=ROOT,
                      stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL).returncode:
        subprocess.run(["git", "fetch", "--no-tags", "origin", "legacy"], cwd=ROOT, check=True)
    if git("rev-parse", archive).decode().strip() != metadata["archiveTree"]:
        raise ValueError("Legacy validation tree does not match its pinned revision")
    destination.parent.mkdir(parents=True, exist_ok=True)
    subprocess.run(["git", "-c", "core.longpaths=true", "clone", "--shared", "--no-checkout",
                    "--quiet", str(ROOT), str(destination)], check=True)
    subprocess.run(["git", "config", "core.longpaths", "true"], cwd=destination, check=True)
    subprocess.run(["git", "checkout", "--quiet", "--detach", revision], cwd=destination, check=True)
    extract_snapshot(git("archive", archive), destination)
    # Replace production directories completely, preserving the archived test code.
    for name in DIRECTORIES:
        path = inside(destination, destination / name)
        if path.exists():
            if path.is_symlink():
                raise ValueError(f"Production directory must not be a symlink: {path}")
            shutil.rmtree(path)
    for name in files:
        target = inside(destination, destination / name)
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / name, target)
    # The resource symlink above may have been materialized before replacing shared/.
    if os.name == "nt":
        resources = destination / "macos/Sources/QuotaBar/Resources"
        shutil.rmtree(inside(destination, resources))
        shutil.copytree(destination / "shared/assets", resources)
    subprocess.run(["git", "remote", "set-url", "origin",
                    git("remote", "get-url", "origin").decode().strip()], cwd=destination, check=True)
    receipt.write_text(json.dumps(marker, indent=2) + "\n", encoding="utf-8")
    if any(hashlib.sha256((destination / name).read_bytes()).hexdigest() != value
           for name, value in hashes.items()):
        raise ValueError("Production overlay verification failed")
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--destination", type=Path)
    parser.add_argument("--script", help="Run a script from the archived validation suite")
    parser.add_argument("args", nargs=argparse.REMAINDER)
    options = parser.parse_args()
    root = prepare(options.destination)
    for variable in ("GITHUB_OUTPUT", "GITHUB_ENV"):
        if os.environ.get(variable):
            key = "root" if variable == "GITHUB_OUTPUT" else "QUOTABAR_VALIDATION_ROOT"
            with open(os.environ[variable], "a", encoding="utf-8") as output:
                output.write(f"{key}={root.as_posix()}\n")
    if not options.script:
        print(root)
        return 0
    script = inside(root, root / options.script)
    if script.suffix not in (".ps1", ".sh", ".py") or not script.is_file():
        raise ValueError("Expected an archived PowerShell, Bash or Python script")
    executable = {".ps1": "pwsh", ".sh": "bash", ".py": sys.executable}[script.suffix]
    args = options.args[1:] if options.args[:1] == ["--"] else options.args
    environment = dict(os.environ)
    local_tools = [ROOT / ".tools/rust/bin", ROOT / ".tools/dotnet"]
    environment["PATH"] = os.pathsep.join(str(p) for p in local_tools if p.is_dir()) + os.pathsep + environment["PATH"]
    if (ROOT / ".tools/cargo").is_dir():
        environment["CARGO_HOME"] = str(ROOT / ".tools/cargo")
    return subprocess.call([executable, str(script), *args], cwd=root, env=environment)


if __name__ == "__main__":
    raise SystemExit(main())
