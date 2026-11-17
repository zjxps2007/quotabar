"""Drive the packaged dashboard through the private macOS monitor protocol."""
import hashlib
import json
import os
from contextlib import ExitStack
from pathlib import Path
import platform
import plistlib
import select
import shutil
import struct
import subprocess
import tempfile
import time
import uuid
import dashboard_actions

ROOT = Path(__file__).resolve().parents[2]
APP = ROOT / "artifacts/QuotaBar.app/Contents/Helpers/QuotaBar Dashboard.app/Contents/MacOS/QuotaBar"
MONITOR = ROOT / "artifacts/QuotaBar.app/Contents/MacOS/QuotaBar"
REPORT = ROOT / "artifacts/macos-dashboard"
ACCOUNT = "92124973-9928-42EE-A7C6-2CC80E4B1972"
THEMES = ("light", "dark")
ROUTES = ("dashboard", "accounts", "settings", "connect", "connect-claude", "connect-gemini")

def fixture_environment():
    directory = REPORT / "paths"
    directory.mkdir(parents=True, exist_ok=True)
    return {"CODEX_HOME": str(directory / "Codex"), "CLAUDE_CONFIG_DIR": str(directory / "Claude")}


def send(pipe, event):
    body = json.dumps(event, ensure_ascii=False).encode()
    pipe.write(struct.pack(">I", len(body)) + body)
    pipe.flush()


def receive(pipe, deadline):
    def exact(n):
        data = bytearray()
        while len(data) < n:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not select.select([pipe], [], [], remaining)[0]:
                raise TimeoutError("The dashboard did not answer")
            chunk = os.read(pipe.fileno(), n - len(data))
            if not chunk:
                raise EOFError("The dashboard closed its private pipe")
            data.extend(chunk)
        return data

    size, = struct.unpack(">I", exact(4))
    if not 0 < size <= 4 * 1024 * 1024:
        raise ValueError("Invalid dashboard frame size")
    return json.loads(exact(size))


def wait_for(pipe, name):
    deadline = time.monotonic() + 20
    while True:
        event = receive(pipe, deadline)
        if name in event:
            return event[name]


def update_receipt(home, updates, cleanup):
    """Set up the helper's existing install.json for the just-started packaged app."""
    bundle = MONITOR.parents[2]
    info = plistlib.loads((bundle / "Contents/Info.plist").read_bytes())
    version = info["CFBundleShortVersionString"]
    archive = ROOT / f"artifacts/QuotaBar-{version}-macOS-{platform.machine()}.zip"
    if not archive.is_file():
        archive = home / archive.name
        subprocess.run(["/usr/bin/ditto", "-c", "-k", "--keepParent", bundle, archive], check=True)
    repository = "https://github.com/zjxps2007/quotabar/releases"
    asset = {"name": archive.name, "bytes": archive.stat().st_size,
             "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
             "url": f"{repository}/download/v{version}/{archive.name}"}
    release = {"tag": f"v{version}", "version": version, "page": f"{repository}/tag/v{version}", "asset": asset}
    directory = updates / str(uuid.uuid4()).upper()
    directory.mkdir(mode=0o700, parents=True)
    cleanup.callback(shutil.rmtree, directory)
    plan = {"target": bundle.as_uri(), "currentVersion": "0.0.0", "staged": {
        "directory": directory.as_uri(), "update": {"build": int(info["CFBundleVersion"]), "release": release, "asset": asset}}}
    (directory / "install.json").write_text(json.dumps(plan))
    return directory, version


def check_monitor():
    with tempfile.TemporaryDirectory() as home, ExitStack() as cleanup:
        home = Path(home).resolve()
        env = {**os.environ, **fixture_environment(), "HOME": str(home), "CFFIXED_USER_HOME": str(home)}
        with subprocess.Popen([MONITOR, "--dashboard-helper", "--demo"], stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, env=env, bufsize=0) as child:
            try:
                wait_for(child.stdout, "ready")
                send(child.stdin, {"close": {}})
                assert child.wait(timeout=7) == 0
            finally:
                if child.poll() is None:
                    child.kill()
        log = home / "monitor.jsonl"
        # The launcher and the installer resolve the current user's Library via
        # Foundation. Overriding HOME doesn't set that directory on every macOS.
        monitor_env = {**os.environ, **fixture_environment()}
        foundation_home = subprocess.check_output(
            ["swift", "-e", "import Foundation; print(FileManager.default.homeDirectoryForCurrentUser.path)"],
            env=monitor_env, text=True, timeout=45).strip()
        updates = Path(foundation_home) / "Library/Application Support/AIUsage Lite/Updates"
        transaction, version = update_receipt(home, updates, cleanup)
        def events():
            if not log.exists():
                return []
            return [json.loads(line) for line in log.read_text().splitlines(keepends=True) if line.endswith("\n")]
        with subprocess.Popen([MONITOR, "--demo", "--menu-bar-only", "--memory-probe", "--diagnostics", log,
                               "--update-ack", transaction.name], env=monitor_env) as child:
            try:
                deadline = time.monotonic() + 35
                while not any(e["event"] == "memory-probe-idle" for e in events()):
                    if child.poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError("Packaged monitor did not finish opening and closing its popover")
                    time.sleep(0.1)
                # Ask just this NSRunningApplication to quit through its AppKit delegate.
                request = f"import AppKit; guard NSRunningApplication(processIdentifier: {child.pid})?.terminate() == true else {{ exit(1) }}"
                subprocess.run(["swift", "-e", request], check=True, timeout=45)
                assert child.wait(timeout=10) == 0
            finally:
                if child.poll() is None:
                    child.kill()
        records = events()
        names = [e["event"] for e in records]
        idle = next(e for e in records if e["event"] == "memory-probe-idle")
        assert "ui-launched" in names and "ui-terminated" in names
        assert names.count("popover-opened") >= 2 and names.count("popover-closed") >= 2
        assert not idle["dashboardRetained"] and not idle["popoverVisible"]
        receipt = transaction / "started.json"
        result = json.loads(receipt.read_bytes())
        assert result["success"] and result["version"] == version and receipt.stat().st_mode & 0o077 == 0
        (REPORT / "monitor.json").write_text(json.dumps(records, indent=2))
        print("PASS packaged monitor launch, update startup acknowledgement, legacy helper forwarding, menu-only lifetime and AppKit termination")


def write_fixtures():
    now = time.time() - 978307200
    state = {"settings": {"accounts": [{"id": ACCOUNT, "name": "개인 계정 · 샘플", "provider": "codex",
                                        "identity": "fixture", "createdAt": now, "enabled": True,
                                        "showInMenuBar": True, "sourcePath": ""}],
                          "autoRefresh": False, "refreshSeconds": 300, "scanLocalLogs": True},
             "snapshots": [ACCOUNT, {"fetchedAt": now, "plan": "샘플",
                                      "windows": [{"title": "5시간", "remaining": 38.5, "resetAt": now + 3600}]}],
             "errors": [], "notes": [], "loading": [],
             "demoReport": {"days": [], "scannedAt": now, "scannedFiles": 0, "warnings": []}}
    for theme in THEMES:
        state["settings"]["appearance"] = theme
        (REPORT / f"fixture-{theme}.json").write_text(json.dumps(state, ensure_ascii=False))


def capture(child, route, path, width=1120, height=810):
    send(child.stdin, {"capture": {"route": route, "path": str(path), "width": width, "height": height}})
    assert wait_for(child.stdout, "captured")["_0"] == str(path)
    png = path.read_bytes()
    assert png[:8] == b"\x89PNG\r\n\x1a\n"
    size = list(struct.unpack(">II", png[16:24]))
    assert size[0] >= 500 and size[1] >= 300
    return size


def check_resize(child, controls, first_path, first_size):
    observations = {}
    captures = REPORT / "resize-captures"
    captures.mkdir(exist_ok=True)
    for name, width, height in (("cold", 1120, 810), ("repeat", 1120, 810),
                                ("smaller", 920, 666), ("restored", 1120, 810)):
        path = first_path if name == "cold" else captures / f"{name}.png"
        size = first_size if name == "cold" else capture(child, "dashboard", path, width, height)
        geometry = controls.request("captureGeometry", value=str(path))
        observations[name] = {"requested": [width, height], "png": size, "image": str(path.relative_to(REPORT)), **geometry}
    (REPORT / "capture-geometry.json").write_text(json.dumps(observations, indent=2))
    for sample in observations.values():
        assert sample["png"] == [round(sample["backing"][k]) for k in ("width", "height")], sample
    assert len(observations["cold"]["statCardColumns"]) == 4, observations
    for name in ("repeat", "restored"):
        sample = observations[name]
        if sample["visibleFrame"] == observations["cold"]["visibleFrame"]:
            assert sample["png"] == first_size, observations
            assert sample["statCardColumns"] == observations["cold"]["statCardColumns"], observations
    assert observations["smaller"]["view"]["width"] < observations["restored"]["view"]["width"], observations


def main():
    REPORT.mkdir(parents=True, exist_ok=True)
    dependencies = {}
    for name, program in (("monitor", MONITOR), ("dashboard", APP),
                          ("statusline", MONITOR.with_name("quotabar-statusline"))):
        linked = subprocess.check_output(["otool", "-L", program], text=True).splitlines()[1:]
        dependencies[name] = [line.strip().split(" (compatibility version", 1)[0] for line in linked]
    (REPORT / "linked-libraries.json").write_text(json.dumps(dependencies, indent=2))
    for name, libraries in dependencies.items():
        assert not any("libswift" in path or "/Combine.framework/" in path or "/SwiftUI.framework/" in path
                       for path in libraries), (name, libraries)
    write_fixtures()
    sizes = {}
    movement = {}
    with tempfile.TemporaryDirectory() as home, ExitStack() as stack:
        library = dashboard_actions.build_driver(Path(home))
        check_monitor()
        child = stack.enter_context(subprocess.Popen(
            [APP, "--dashboard-helper", "--demo"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            env={**os.environ, **fixture_environment(), "HOME": home, "CFFIXED_USER_HOME": home,
                 "DYLD_INSERT_LIBRARIES": str(library), "QUOTABAR_TEST_UI_DIRECTORY": home}, bufsize=0,
        ))
        controls = dashboard_actions.Controls(Path(home), child)
        try:
            wait_for(child.stdout, "ready")
            for theme in THEMES:
                state = json.loads((REPORT / f"fixture-{theme}.json").read_text())
                send(child.stdin, {"state": {"_0": state}})
                for page in ("settings", "dashboard"):
                    send(child.stdin, {"show": {"route": page}})
                    assert wait_for(child.stdout, "route") == {"_0": page}
                for route in ROUTES:
                    path = REPORT / f"{route}-{theme}.png"
                    sizes[path.name] = capture(child, route, path)
                    movement[path.name] = controls.request("windowMovement", expected={
                        "movable": False, "backgroundMovable": False,
                    })
                    if route == "dashboard" and theme == THEMES[0]:
                        check_resize(child, controls, path, sizes[path.name])
            send(child.stdin, {"close": {}})
            assert child.wait(timeout=7) == 0
        finally:
            if child.poll() is None:
                child.kill()
    (REPORT / "result.json").write_text(json.dumps(sizes, indent=2))
    (REPORT / "window-movement.json").write_text(json.dumps(movement, indent=2))
    print(f"PASS {len(sizes)} packaged macOS dashboard and dialog captures, handshake, routing and shutdown")


if __name__ == "__main__":
    main()
