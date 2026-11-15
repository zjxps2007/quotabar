"""Compare the actual AppKit menu, connection sheet and monitor commands on both helpers."""
import json
import os
from pathlib import Path
import shutil
import select
from copy import deepcopy
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

        def expect_no_command(name):
            deadline = time.monotonic() + 0.6
            while (remaining := deadline - time.monotonic()) > 0:
                if not select.select([app.stdout], [], [], remaining)[0]:
                    return
                if name in dashboard.receive(app.stdout, deadline):
                    raise ValueError(f"Unexpected {name} command")

        def route(name):
            value = command("route")["_0"]
            if value != name:
                raise ValueError(f"Unexpected route {value!r}, expected {name!r}")
            observed.append({"route": value})

        def menu(title, event, destination=None, via="menu"):
            item = controls.request(via, title)
            payload = command(event)
            if destination and payload != {"_0": destination}:
                raise ValueError(f"{title}: {payload}")
            observed.append({via: item, "event": {event: payload}})

        def preference(label, event, key, value):
            controls.request("press", label)
            reply = command(event)
            if reply != {"_0": value}:
                raise ValueError(f"{label}: expected {value}, got {reply}")
            fixture["settings"][key] = value
            send({"state": {"_0": fixture}})
            if event == "appearance":
                controls.request("appearance", expected={"appearance": value})
            observed.append({"preference": event, "value": value})
            time.sleep(0.25)
            # Account for any repeated command from this click before sending
            # the next one. Keep duplicates in the comparison and its evidence.
            while select.select([app.stdout], [], [], 0)[0]:
                extra = dashboard.receive(app.stdout, time.monotonic() + 2)
                if event in extra:
                    observed.append({"repeatedPreference": event, "value": extra[event]["_0"]})

        consent = "이 폴더의 statusLine 연결 허용 (기존 명령은 보존)"
        try:
            command("ready")
            send({"state": {"_0": fixture}})
            send({"show": {"route": "accounts"}})
            route("accounts")
            menu("새로고침", "refreshAll", via="shortcut")
            menu("상단바 사용량 보기", "showPopover")
            menu("설정…", "route", "settings", via="shortcut")
            time.sleep(0.4)
            preference("다크", "appearance", "appearance", "dark")
            preference("라이트", "appearance", "appearance", "light")
            preference("아이콘만", "menuBarMode", "menuBarDisplayMode", "iconOnly")
            preference("이름 + 잔여량", "menuBarMode", "menuBarDisplayMode", "providerAndRemaining")
            preference("Dock에 앱 아이콘 표시", "dockIcon", "showDockIcon", False)
            preference("Dock에 앱 아이콘 표시", "dockIcon", "showDockIcon", True)
            fixture["settings"]["autoRefresh"] = True
            send({"state": {"_0": fixture}})
            time.sleep(0.3)
            selection = controls.request("intervalSelect", "15분")
            if (selection["choices"] != ["1분", "5분", "15분", "30분", "60분"] or
                    selection["checked"] != ["5분"] or command("interval")["_0"] != 900):
                raise ValueError(f"Unexpected interval selection: {selection}")
            observed.append({"intervalSelection": selection})
            fixture["settings"]["refreshSeconds"] = 900
            send({"state": {"_0": fixture}})
            time.sleep(0.3)
            dismissed = controls.request("intervalCancel", "5분")
            if dismissed["checked"] != ["15분"] or dismissed["selection"] is not None:
                raise ValueError(f"Unexpected dismissed interval: {dismissed}")
            expect_no_command("interval")
            observed.append({"intervalDismissed": dismissed})
            fixture["settings"].update(autoRefresh=False, refreshSeconds=300)
            send({"state": {"_0": fixture}})
            menu("Dashboard 열기", "route", "dashboard")
            send({"show": {"route": "accounts"}})
            route("accounts")
            time.sleep(0.35)
            for previous, provider in (("전체 Provider", "Gemini"), ("Gemini", "전체 Provider")):
                selection = controls.request("providerSelect", provider)
                if selection["checked"] != [previous] or selection["choices"] != ["전체 Provider", "Codex", "Claude", "Gemini"]:
                    raise ValueError(f"Unexpected provider selection: {selection}")
                observed.append({"providerFilter": selection})
                time.sleep(0.20)
            account = fixture["settings"]["accounts"][0]
            for enabled in (False, True):
                controls.request("press", f"Codex({account['name']}) 모니터링")
                if UUID(command("toggle")["_0"]) != UUID(account["id"]):
                    raise ValueError("Monitoring switch targeted a different account")
                account["enabled"] = enabled
                send({"state": {"_0": fixture}})
                observed.append({"monitoring": enabled})
                time.sleep(0.25)
            controls.request("press", "계정 연결")
            controls.request("sheet", expected={"sheet": True})
            observed.append({"connectionSheet": "opened"})
            for segment, provider, path in (
                ("파일 가져오기", "codex", Path(env["CODEX_HOME"]) / "auth.json"),
                ("Gemini", "gemini", Path.home() / ".gemini" / "oauth_creds.json"),
            ):
                controls.request("press", segment)
                time.sleep(0.35)
                controls.request("press", "연결")
                request = command("connect")
                request_id = request.pop("id")
                UUID(request_id)
                if request != {"provider": provider, "path": str(path), "alias": ""}:
                    raise ValueError(f"Unexpected {provider} file connection {request}")
                observed.append({"fileConnection": request})
                send({"reply": {"id": request_id, "error": "검증용 연결 오류"}})
                time.sleep(0.35)
            controls.request("press", "Claude")
            time.sleep(0.35)
            controls.request("attempt", "연결")
            expect_no_command("connect")
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
            controls.request("accountRefresh", f"{account['name']} 계정 메뉴", {"selected": "새로고침"})
            if UUID(command("refresh")["_0"]) != UUID(account["id"]):
                raise ValueError("The account menu refreshed a different account")
            observed.append({"accountMenuRefresh": True})
            for button in ("취소", "삭제"):
                controls.request("accountRemove", f"{account['name']} 계정 메뉴", {"selected": "계정 삭제…"})
                controls.request("sheet", expected={"sheet": True})
                expect_no_command("remove")
                controls.request("press", button)
                controls.request("sheet", expected={"sheet": False})
                if button == "취소":
                    expect_no_command("remove")
                    observed.append({"removeCancelled": True})
                else:
                    removed = UUID(command("remove")["_0"])
                    if removed != UUID(account["id"]):
                        raise ValueError(f"The confirmation removed a different account: {removed}")
                    expect_no_command("remove")
                    observed.append({"removeConfirmed": str(removed)})
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
                    "-framework", "AppKit", "-framework", "ScreenCaptureKit", "-framework", "CoreGraphics", "-framework", "Vision",
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
                if route in ("dashboard", "connect-claude", "remove"):
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
            account = fixture["settings"]["accounts"][0]
            controls.request("accountRemove", f"{account['name']} 계정 메뉴", {"selected": "계정 삭제…"})
            controls.request("sheet", expected={"sheet": True})
            capture("remove", sheet=True)
            controls.request("press", "취소")
            controls.request("sheet", expected={"sheet": False})
            show("settings")
            time.sleep(0.35)
            for _ in range(3):
                controls.request("scrollSettings")
                time.sleep(0.30)
            capture("settings-bottom")
            controls.request("press", "라이선스 및 고지 보기")
            controls.request("sheet", expected={"sheet": True})
            journey = []
            for title, scene, document in ((None, "licenses-notice", "NOTICE"),
                                           ("Apache License 2.0 전문", "licenses-apache", "LICENSE"),
                                           ("저작권 및 변경 고지", "licenses-notice", "NOTICE")):
                if title:
                    controls.request("press", title)
                if document == "NOTICE" and journey:
                    time.sleep(0.6)
                    controls.request("snapshot", "sheet")
                else:
                    capture(scene, sheet=True)
                    if "error" in observed[scene]:
                        raise ValueError(observed[scene]["error"])
                journey.append(controls.request("document", expected={"document": document})["document"])
            controls.request("press", "닫기")
            controls.request("sheet", expected={"sheet": False})
            observed["licenseDocuments"] = [*journey, "closed"]
            controls.request("press", "라이선스 및 고지 보기")
            controls.request("sheet", expected={"sheet": True})
            controls.request("press", "문서 본문")
            time.sleep(0.2)
            controls.request("documentEscape")
            controls.request("sheet", expected={"sheet": False})
            observed["licenseEscapeClosed"] = True
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
        for route in (*dashboard.ROUTES, "remove", "popover-sample", "popover-empty", "popover-overflow",
                      "settings-bottom", "licenses-notice", "licenses-apache"):
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
    documents_match = all(results[side][theme].get("licenseDocuments") == ["NOTICE", "LICENSE", "NOTICE", "closed"]
                          and results[side][theme].get("licenseEscapeClosed") is True
                          for side in ("original", "rust") for theme in dashboard.THEMES)
    return documents_match and all(row["identical"] for row in results["comparisons"].values())


def compare(dashboard, work, original, fixture, library):
    fixture = {**fixture, "settings": {**fixture["settings"], "scanLocalLogs": False}}
    results = {}
    for side, program in (("original", original), ("rust", dashboard.APP)):
        record = {"observed": []}
        results[side] = record
        try:
            exercise(dashboard, program, work / side, library, deepcopy(fixture), record["observed"])
        except (OSError, ValueError, KeyError, RuntimeError, EOFError, subprocess.SubprocessError) as error:
            record["error"] = str(error)
        print(f"{side}: {len(record['observed'])} dashboard actions; {record.get('error', 'complete')}", flush=True)
    results["identical"] = (results["original"] == results["rust"] and "error" not in results["original"])
    # Keep the full traffic above, including duplicate switch notifications.
    # A user action completes only after its expected command and resulting state are verified.
    def outcomes(side):
        return [action for action in results[side]["observed"] if "repeatedPreference" not in action]
    results["userActionsIdentical"] = (all("error" not in results[side] for side in ("original", "rust"))
                                       and outcomes("original") == outcomes("rust"))
    (dashboard.REPORT / "actions.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
    return results["identical"]
