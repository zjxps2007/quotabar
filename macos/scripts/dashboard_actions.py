"""Compare the actual AppKit menu, connection sheet and monitor commands on both helpers."""
import json
import os
from pathlib import Path
import shutil
import select
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
    if executable != dashboard.APP:
        env["QUOTABAR_TEST_REFERENCE_INPUT"] = "1"
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
            time.sleep(0.35)
            controls.request("press", "계정 연결")
            controls.request("sheet", expected={"sheet": True})
            observed.append({"connectionSheet": "opened"})
            controls.request("press", "Claude")
            time.sleep(0.35)
            controls.request("attempt", "연결")
            deadline = time.monotonic() + 0.6
            while time.monotonic() < deadline and select.select([app.stdout], [], [], max(0, deadline-time.monotonic()))[0]:
                if "connect" in dashboard.receive(app.stdout, deadline):
                    raise ValueError("Claude submitted without consent")
            controls.request("press", consent)
            time.sleep(0.25)
            controls.request("press", "연결")
            request = command("connect")
            request_id = request.pop("id")
            UUID(request_id)
            expected = {"provider": "claude", "path": env["CLAUDE_CONFIG_DIR"], "alias": ""}
            if request != expected:
                raise ValueError(f"Invalid Claude connection command {request}")
            observed.append({"claudeConsentEnablesSubmit": True})
            observed.append({"connect": request})
            send({"reply": {"id": request_id, "error": "검증용 연결 오류"}})
            time.sleep(0.35)
            controls.request("sheet", expected={"sheet": True})
            controls.request("press", "취소")
            controls.request("sheet", expected={"sheet": False})
            observed.append({"failedConnectionCanBeCancelled": True})
            controls.request("press", "계정 연결")
            controls.request("sheet", expected={"sheet": True})
            controls.request("press", "Claude")
            time.sleep(0.35)
            controls.request("press", consent)
            time.sleep(0.25)
            controls.request("press", "연결")
            next_request = command("connect")
            next_id = next_request.pop("id")
            if UUID(next_id) == UUID(request_id) or next_request != expected:
                raise ValueError(f"Unexpected second connection request {next_request}")
            send({"reply": {"id": next_id}})
            controls.request("sheet", expected={"sheet": False})
            observed.append({"successfulConnectionDismissesSheet": True})
            menu("QuotaBar 종료", "quit")
            send({"close": {}})
            if app.wait(timeout=8) != 0:
                raise RuntimeError("Dashboard did not exit normally")
            observed.append({"closed": True})
        finally:
            if app.poll() is None:
                app.kill()


def build_driver(work):
    library = work / "dashboard-ui-driver.dylib"
    subprocess.run(["xcrun", "clang", "-mmacosx-version-min=14.4", "-fobjc-arc", "-fblocks", "-dynamiclib",
                    "-framework", "AppKit", "-framework", "ScreenCaptureKit", "-framework", "CoreGraphics",
                    Path(__file__).with_name("dashboard-ui-driver.m"), "-o", library], check=True)
    return library


def capture_windows(dashboard, program, folder, library, fixture, output, theme):
    folder.mkdir()
    env = {**os.environ, **dashboard.fixture_environment(), "DYLD_INSERT_LIBRARIES": str(library),
           "QUOTABAR_TEST_UI_DIRECTORY": str(folder)}
    if program != dashboard.APP:
        env["QUOTABAR_TEST_REFERENCE_INPUT"] = "1"
    observed = {}
    with subprocess.Popen([program, "--dashboard-helper", "--demo"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          env=env, bufsize=0) as app:
        controls = Controls(folder, app)

        def show(route):
            dashboard.send(app.stdin, {"show": {"route": route}})
            value = dashboard.wait_for(app.stdout, "route")
            if value.get("_0") != route:
                raise ValueError(f"Expected route {route}, got {value}")

        def capture(route, sheet=False):
            time.sleep(0.7)  # Allow the AppKit sheet/route transition to finish before capturing.
            try:
                observed[route] = controls.request("snapshot", "sheet" if sheet else "dashboard")
                shutil.copyfile(folder / "frame.png", output / f"{route}-{theme}.png")
                if route in ("dashboard", "connect-claude"):
                    # A second frame from the untouched window measures e.g. caret blinking.
                    time.sleep(0.55)
                    controls.request("snapshot", "sheet" if sheet else "dashboard")
                    repeat = output / "repeat"
                    repeat.mkdir(exist_ok=True)
                    shutil.copyfile(folder / "frame.png", repeat / f"{route}-{theme}.png")
            except (ValueError, OSError) as error:
                observed[route] = {"error": str(error)}

        try:
            dashboard.wait_for(app.stdout, "ready")
            dashboard.send(app.stdin, {"state": {"_0": fixture}})
            for route in ("accounts", "dashboard", "settings"):
                show(route)
                if route == "accounts":
                    controls.request("size")
                capture(route)
            show("accounts")
            time.sleep(0.35)
            controls.request("press", "계정 연결")
            controls.request("sheet", expected={"sheet": True})
            capture("connect", sheet=True)
            for name in ("Claude", "Gemini"):
                controls.request("press", name)
                capture(f"connect-{name.lower()}", sheet=True)
            controls.request("press", "취소")
            controls.request("sheet", expected={"sheet": False})
        except (OSError, ValueError, KeyError, RuntimeError, EOFError, subprocess.SubprocessError) as error:
            observed["error"] = str(error)
        finally:
            if app.poll() is None:
                try:
                    dashboard.send(app.stdin, {"close": {}})
                    app.wait(timeout=4)
                except (OSError, subprocess.TimeoutExpired):
                    app.kill()
    return observed


def compare_windows(dashboard, work, original, library):
    dest = dashboard.REPORT / "windows"
    results = {"original": {}, "rust": {}, "comparisons": {}, "repeatability": {}}
    for side, program in (("original", original), ("rust", dashboard.APP)):
        output = dest / side
        output.mkdir(parents=True, exist_ok=True)
        for theme in dashboard.THEMES:
            fixture = json.loads((dashboard.REPORT / f"fixture-{theme}.json").read_text())
            results[side][theme] = capture_windows(dashboard, program, work / f"windows-{side}-{theme}",
                                                  library, fixture, output, theme)
    for theme in dashboard.THEMES:
        for route in (*dashboard.ROUTES, "popover-sample", "popover-empty", "popover-overflow"):
            name = f"{route}-{theme}"
            lhs, rhs = (dest / side / f"{name}.png" for side in ("original", "rust"))
            if not lhs.is_file() or not rhs.is_file():
                results["comparisons"][name] = {"identical": False, "error": "Missing window capture"}
                continue
            result = work / "visible-pixels.json"
            try:
                subprocess.run([original, "--pixels", lhs, rhs, result, "--reference-ui"], check=True, timeout=20)
                results["comparisons"][name] = json.loads(result.read_text())
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                results["comparisons"][name] = {"identical": False, "error": str(error)}
    for side in ("original", "rust"):
        results["repeatability"][side] = {}
        for repeated in sorted((dest / side / "repeat").glob("*.png")):
            result = work / "repeat-pixels.json"
            subprocess.run([original, "--pixels", repeated, dest / side / repeated.name, result, "--reference-ui"],
                           check=True, timeout=20)
            results["repeatability"][side][repeated.stem] = json.loads(result.read_text())
    (dest / "comparison.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
    return all(row["identical"] for row in results["comparisons"].values())


def compare(dashboard, work, original, fixture, library):
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
