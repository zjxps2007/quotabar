"""Compare the actual AppKit menu, connection sheet and monitor commands on both helpers."""
import json
import os
from pathlib import Path
import subprocess
import time
from uuid import UUID


class Controls:
    def __init__(self, folder, child):
        self.folder = folder
        self.child = child
        self.sequence = 0

    def request(self, op, label="", expected=None):
        deadline = time.monotonic() + 8
        result = None
        while time.monotonic() < deadline:
            self.sequence += 1
            request = {"id": self.sequence, "op": op, "label": label}
            tmp = self.folder / "request.tmp"
            tmp.write_text(json.dumps(request, ensure_ascii=False))
            tmp.replace(self.folder / "request.json")
            while time.monotonic() < deadline:
                if self.child.poll() is not None:
                    raise RuntimeError(f"dashboard exited during {op} {label} ({self.child.returncode})")
                reply = self.folder / "reply.json"
                if reply.exists():
                    message = json.loads(reply.read_bytes())
                    if message["id"] == self.sequence:
                        result = message["result"]
                        break
                time.sleep(0.04)
            if result and "error" in result:
                raise ValueError(result)
            if result and "pending" not in result and (expected is None or result == expected):
                return result
            time.sleep(0.05)
        raise TimeoutError(f"{op} {label}: expected {expected}, observed {result}")


def exercise(dashboard, executable, folder, library, fixture, observed):
    folder.mkdir()
    env = {**os.environ, **dashboard.fixture_environment(), "HOME": str(folder),
           "DYLD_INSERT_LIBRARIES": str(library), "QUOTABAR_TEST_UI_DIRECTORY": str(folder)}
    with subprocess.Popen([executable, "--dashboard-helper"], stdin=subprocess.PIPE,
                          stdout=subprocess.PIPE, bufsize=0, env=env) as app:
        controls = Controls(folder, app)

        def send(event):
            dashboard.send(app.stdin, event)

        def command(name):
            return dashboard.wait_for(app.stdout, name)

        def route(name):
            value = command("route")["_0"]
            if value != name:
                raise ValueError(f"Unexpected route {value!r}, expected {name!r}")
            observed.append({"route": value})

        def menu(title, event, destination=None):
            item = controls.request("menu", title)
            payload = command(event)
            if destination and payload != {"_0": destination}:
                raise ValueError(f"{title}: {payload}")
            observed.append({"menu": item, "event": {event: payload}})

        consent = "이 폴더의 statusLine 연결 허용 (기존 명령은 보존)"
        try:
            command("ready")
            send({"state": {"_0": fixture}})
            send({"show": {"route": "accounts"}})
            route("accounts")
            menu("새로고침", "refreshAll")
            menu("상단바 사용량 보기", "showPopover")
            menu("설정…", "route", "settings")
            menu("Dashboard 열기", "route", "dashboard")
            send({"show": {"route": "accounts"}})
            route("accounts")
            controls.request("press", "계정 연결")
            controls.request("sheet", expected={"sheet": True})
            observed.append({"connectionSheet": "opened"})
            controls.request("press", "Claude")
            controls.request("enabled", "연결", {"enabled": False})
            controls.request("press", consent)
            controls.request("enabled", "연결", {"enabled": True})
            observed.append({"claudeConsentEnablesSubmit": True})
            controls.request("press", "연결")
            request = command("connect")
            request_id = request.pop("id")
            UUID(request_id)
            expected = {"provider": "claude", "path": env["CLAUDE_CONFIG_DIR"], "alias": ""}
            if request != expected:
                raise ValueError(f"Invalid Claude connection command {request}")
            observed.append({"connect": request})
            send({"reply": {"id": request_id, "error": "검증용 연결 오류"}})
            controls.request("enabled", "연결", {"enabled": True})
            controls.request("sheet", expected={"sheet": True})
            controls.request("press", "취소")
            controls.request("sheet", expected={"sheet": False})
            observed.append({"failedConnectionCanBeCancelled": True})
            menu("QuotaBar 종료", "quit")
            send({"close": {}})
            if app.wait(timeout=8) != 0:
                raise RuntimeError("Dashboard did not exit normally")
            observed.append({"closed": True})
        finally:
            if app.poll() is None:
                app.kill()


def compare(dashboard, work, original, fixture):
    library = work / "dashboard-ui-driver.dylib"
    subprocess.run(["xcrun", "clang", "-fobjc-arc", "-fblocks", "-dynamiclib", "-framework", "AppKit",
                    Path(__file__).with_name("dashboard-ui-driver.m"), "-o", library], check=True)
    fixture = {**fixture, "settings": {**fixture["settings"], "scanLocalLogs": False}}
    results = {}
    for side, program in (("original", original), ("rust", dashboard.APP)):
        record = {"observed": []}
        results[side] = record
        try:
            exercise(dashboard, program, work / side, library, fixture, record["observed"])
        except (OSError, ValueError, KeyError, RuntimeError, EOFError, subprocess.SubprocessError) as error:
            record["error"] = str(error)
        print(f"{side}: {len(record['observed'])} dashboard actions; {record.get('error', 'complete')}", flush=True)
    results["identical"] = (results["original"] == results["rust"] and "error" not in results["original"])
    (dashboard.REPORT / "actions.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
    return results["identical"]
