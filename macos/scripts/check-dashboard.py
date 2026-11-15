"""Drive the packaged dashboard through the private macOS monitor protocol."""
import json
import os
from pathlib import Path
import select
import struct
import subprocess
import tempfile
import time

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


def check_monitor():
    with tempfile.TemporaryDirectory() as home:
        env = {**os.environ, **fixture_environment(), "HOME": home}
        with subprocess.Popen([MONITOR, "--dashboard-helper", "--demo"], stdin=subprocess.PIPE,
                              stdout=subprocess.PIPE, env=env, bufsize=0) as child:
            try:
                wait_for(child.stdout, "ready")
                send(child.stdin, {"close": {}})
                assert child.wait(timeout=7) == 0
            finally:
                if child.poll() is None:
                    child.kill()
        log = Path(home) / "monitor.jsonl"
        def events():
            if not log.exists():
                return []
            return [json.loads(line) for line in log.read_text().splitlines(keepends=True) if line.endswith("\n")]
        with subprocess.Popen([MONITOR, "--demo", "--menu-bar-only", "--memory-probe", "--diagnostics", log], env=env) as child:
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
        (REPORT / "monitor.json").write_text(json.dumps(records, indent=2))
        print("PASS packaged monitor launch, legacy helper forwarding, menu-only lifetime and AppKit termination")


def main():
    REPORT.mkdir(parents=True, exist_ok=True)
    check_monitor()
    now = time.time() - 978307200
    state = {"settings": {"accounts": [{"id": ACCOUNT, "name": "개인 계정 · 샘플", "provider": "codex",
                                        "identity": "fixture", "createdAt": now, "enabled": True,
                                        "showInMenuBar": True, "sourcePath": ""}],
                          "autoRefresh": False, "refreshSeconds": 300, "scanLocalLogs": True},
             "snapshots": [ACCOUNT, {"fetchedAt": now, "plan": "샘플",
                                      "windows": [{"title": "5시간", "remaining": 38.5, "resetAt": now + 3600}]}],
             "errors": [], "notes": [], "loading": [],
             "demoReport": {"days": [], "scannedAt": now, "scannedFiles": 0, "warnings": []}}
    sizes = {}
    with tempfile.TemporaryDirectory() as home, subprocess.Popen(
        [APP, "--dashboard-helper", "--demo"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        env={**os.environ, **fixture_environment(), "HOME": home}, bufsize=0,
    ) as child:
        try:
            wait_for(child.stdout, "ready")
            for theme in THEMES:
                state["settings"]["appearance"] = theme
                (REPORT / f"fixture-{theme}.json").write_text(json.dumps(state, ensure_ascii=False))
                send(child.stdin, {"state": {"_0": state}})
                send(child.stdin, {"show": {"route": "dashboard"}})
                wait_for(child.stdout, "route")
                for route in ROUTES:
                    path = REPORT / f"{route}-{theme}.png"
                    send(child.stdin, {"capture": {"route": route, "path": str(path), "width": 1120, "height": 810}})
                    assert wait_for(child.stdout, "captured")["_0"] == str(path)
                    png = path.read_bytes()
                    assert png[:8] == b"\x89PNG\r\n\x1a\n"
                    width, height = struct.unpack(">II", png[16:24])
                    assert width >= 500 and height >= 300
                    sizes[path.name] = [width, height]
            send(child.stdin, {"close": {}})
            assert child.wait(timeout=7) == 0
        finally:
            if child.poll() is None:
                child.kill()
    (REPORT / "result.json").write_text(json.dumps(sizes, indent=2))
    print(f"PASS {len(sizes)} packaged macOS dashboard and dialog captures, handshake, routing and shutdown")


if __name__ == "__main__":
    main()
