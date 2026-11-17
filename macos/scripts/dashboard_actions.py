"""Compare the actual AppKit menu, connection sheet and monitor commands on both helpers."""
import json
import os
from pathlib import Path
import shutil
import select
from copy import deepcopy
from datetime import datetime, timedelta
import subprocess
import time
from uuid import UUID


BANNER_MESSAGE = "샘플 계정의 연결을 다시 확인하세요."
ACCOUNT_EMAIL = "account-selection@example.test"
SOURCE_PATH = "/tmp/QuotaBar 샘플/auth.json"
DATA_PATH = str(Path.home() / "Library/Application Support/AIUsage Lite")
CONNECTION_ERROR = "검증용 연결 오류"
FILE_ERROR_TEXT = "[connection] " + CONNECTION_ERROR
TIMEOUT_TEXT = "[dashboard_timeout] 연결 결과를 받지 못했습니다. 계정 목록을 확인한 뒤 다시 시도하세요."
EXITED_LOGIN_TEXT = "로그인이 완료되지 않았습니다. 다른 Codex 로그인 창을 닫고 다시 시도하세요."
PROVIDERS = ("codex", "claude", "gemini")


class Controls:
    def __init__(self, folder, child):
        self.folder = folder
        self.child = child
        self.sequence = 0

    def request(self, op, label="", expected=None, timeout=8, value=None):
        deadline = time.monotonic() + timeout
        result = None
        while time.monotonic() < deadline:
            self.sequence += 1
            request = {"id": self.sequence, "op": op, "label": label}
            if value is not None:
                request["value"] = value
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


def commands_after_input(dashboard, stream, name):
    events = []
    deadline = time.monotonic() + 3
    while (remaining := deadline - time.monotonic()) > 0:
        if not select.select([stream], [], [], remaining)[0]:
            break
        message = dashboard.receive(stream, deadline)
        if name in message:
            events.append(message[name])
            deadline = time.monotonic() + 0.4
    return events


def exercise(dashboard, executable, folder, library, fixture, observed, help_text):
    folder.mkdir()
    env = {**os.environ, **dashboard.fixture_environment(), "HOME": str(folder),
           "DYLD_INSERT_LIBRARIES": str(library), "QUOTABAR_TEST_UI_DIRECTORY": str(folder),
           "QUOTABAR_TEST_PRIVATE_PASTEBOARD": "1"}
    if executable != dashboard.APP:
        env["QUOTABAR_TEST_REFERENCE_INPUT"] = "1"
    # Exercise an unsuccessful browser login without starting a real CLI or browser.
    fake_cli = folder / "codex"
    fake_cli.write_text("#!/bin/sh\nexit 1\n")
    fake_cli.chmod(0o700)
    env["PATH"] = str(folder) + os.pathsep + env["PATH"]
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
            account = fixture["settings"]["accounts"][0]
            tray_label = f"Codex({account['name']}) 상단바에 표시"
            time.sleep(0.35)
            help_text["tray"] = controls.request("help", tray_label)
            send({"show": {"route": "accounts"}})
            route("accounts")
            time.sleep(0.35)
            search_hint = "계정 이름 또는 이메일 검색"
            monitor_label = f"Codex({account['name']}) 모니터링"
            monitor_help = {"help": "끄면 이 계정의 조회를 일시 정지합니다.", "found": True}
            for term, matches in ((account["name"], True), ("찾을 수 없는 계정 🧪", False)):
                controls.request("focusField", search_hint)
                edited_search = controls.request("editField", search_hint, value=term)
                if edited_search["value"] != term or edited_search["marked"]:
                    raise ValueError(f"Search composition did not commit: {edited_search}")
                if matches:
                    help_text["searchFields"] = controls.request("editableFields")
                controls.request("help", monitor_label, monitor_help if matches else {"help": None, "found": False})
                observed.append({"searchMatchesAccount": matches})
            menu("Dashboard 열기", "route", "dashboard")
            controls.request("fieldValue", search_hint, {"found": False})
            send({"show": {"route": "accounts"}})
            route("accounts")
            controls.request("fieldValue", search_hint, {"value": ""})
            controls.request("help", monitor_label, monitor_help)
            observed.append({"reopeningAccountsClearsSearch": True})
            for previous, provider in (("전체 Provider", "Gemini"), ("Gemini", "전체 Provider")):
                selection = controls.request("providerSelect", provider)
                if selection["checked"] != [previous] or selection["choices"] != ["전체 Provider", "Codex", "Claude", "Gemini"]:
                    raise ValueError(f"Unexpected provider selection: {selection}")
                observed.append({"providerFilter": selection})
                time.sleep(0.20)
            help_text["monitoring"] = controls.request("help", f"Codex({account['name']}) 모니터링")
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
            controls.request("press", "ChatGPT로 로그인")
            controls.request("help", EXITED_LOGIN_TEXT, {"found": True, "help": None})
            observed.append({"browserLoginExited": EXITED_LOGIN_TEXT})
            for segment, provider, path in (
                ("파일 가져오기", "codex", Path(env["CODEX_HOME"]) / "auth.json"),
                ("Gemini", "gemini", Path.home() / ".gemini" / "oauth_creds.json"),
            ):
                controls.request("press", segment)
                time.sleep(0.35)
                if provider == "gemini":
                    help_text["connectionFields"] = controls.request("editableFields")
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
            send({"reply": {"id": request_id, "error": CONNECTION_ERROR}})
            time.sleep(0.35)
            controls.request("sheet", expected={"sheet": True})
            try:
                controls.request("selectText", FILE_ERROR_TEXT)
                controls.request("selectedText", expected={"text": FILE_ERROR_TEXT})
                controls.request("copySelection")
                help_text["connectionErrorClipboard"] = controls.request("selectionClipboard")
            except (TimeoutError, ValueError) as error:
                help_text["connectionErrorCopyError"] = str(error)
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
            controls.request("help", TIMEOUT_TEXT, {"found": True, "help": None}, timeout=38)
            send({"reply": {"id": next_id}})
            time.sleep(0.35)
            controls.request("sheet", expected={"sheet": True})
            controls.request("press", "연결")
            retry = command("connect")
            retry_id = retry.pop("id")
            if UUID(retry_id) == UUID(next_id) or retry != expected:
                raise ValueError(f"Unexpected connection after timeout {retry}")
            observed.append({"lateReplyIgnoredAndRetryAvailable": TIMEOUT_TEXT})
            send({"reply": {"id": retry_id}})
            controls.request("sheet", expected={"sheet": False})
            observed.append({"successfulConnectionDismissesSheet": True})
            controls.request("press", "계정 연결")
            controls.request("sheet", expected={"sheet": True})
            controls.request("press", "Gemini")
            alias_hint = "예: 개인 계정, 연구실"
            alias, path = "검증 계정 🧪", "/tmp/QuotaBar 검증/인증.json"
            for hint, value in ((alias_hint, alias), ("경로", path)):
                controls.request("focusField", hint)
                edited_field = controls.request("editField", hint, value=value)
                if edited_field != {"value": value, "marked": False, "caret": [0, 0, 0]}:
                    raise ValueError(f"Unexpected light-mode field after composition: {edited_field}")
            controls.request("fieldKey", "경로", value="Return")
            edited = command("connect")
            edit_id = edited.pop("id")
            UUID(edit_id)
            if edited != {"provider": "gemini", "path": path, "alias": alias}:
                raise ValueError(f"Unexpected connection after editing both native fields: {edited}")
            observed.append({"composedTextAndReturn": edited})
            fields = controls.request("editableFields")["fields"]
            enabled = {field["placeholder"]: field["enabled"] for field in fields}
            if enabled != {alias_hint: False, "경로": True}:
                raise ValueError(f"Unexpected editors during a pending connection: {fields}")
            observed.append({"pendingConnectionFields": enabled})
            send({"reply": {"id": edit_id}})
            controls.request("sheet", expected={"sheet": False})
            controls.request("press", "계정 연결")
            controls.request("sheet", expected={"sheet": True})
            controls.request("focusField", alias_hint)
            controls.request("fieldKey", alias_hint, value="Escape")
            controls.request("sheet", expected={"sheet": False})
            expect_no_command("connect")
            observed.append({"escapeFromFieldDismissesSheet": True})
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


def activity_fixtures(fixture):
    empty = deepcopy(fixture)
    empty["settings"]["accounts"] = []
    empty["snapshots"] = []
    yield "empty", empty

    state = deepcopy(empty)
    for i in range(6):
        provider = ("codex", "claude")[i % 2]
        key = str(UUID(int=i + 1))
        state["settings"]["accounts"].append({
            **fixture["settings"]["accounts"][0], "id": key, "provider": provider,
            "name": f"{provider.title()} {i + 1}", "identity": key,
        })
        state["snapshots"].extend([key, fixture["snapshots"][1]])
    yield "activity-empty", state

    state = deepcopy(state)
    report = state["demoReport"]
    today = datetime.fromtimestamp(report["scannedAt"] + 978307200).date()
    report["scannedFiles"] = 6
    for provider, amounts in (("claude", (100, 220, 240)), ("codex", (240, 270, 300))):
        for days_ago, total in zip((57, 23, 1), amounts):
            report["days"].append({"day": (today - timedelta(days=days_ago)).isoformat(),
                                   "provider": provider, "model": "Claude Opus" if provider == "claude" else "GPT-5",
                                   "counts": {"input": total - 20, "cached": 10, "output": 20}})
    yield "activity", state


def scroll_activity_to_bottom(controls):
    for _ in range(3):
        controls.request("scrollContent")
        time.sleep(0.30)


def quota_status_fixture(fixture):
    state = deepcopy(fixture)
    paused = state["settings"]["accounts"][0]
    paused["enabled"] = False
    key = str(UUID(int=2))
    state["settings"]["accounts"].append({
        **paused, "id": key, "identity": key, "name": "Claude 대기 계정", "provider": "claude", "enabled": True,
    })
    state["snapshots"].extend([key, {**state["snapshots"][1], "windows": [], "state": "waiting"}])
    state["errors"] = [key, CONNECTION_ERROR]
    return state


def capture_windows(dashboard, program, folder, library, fixture, output, theme, pixel_reader):
    folder.mkdir()
    env = {**os.environ, **dashboard.fixture_environment(), "DYLD_INSERT_LIBRARIES": str(library),
           "QUOTABAR_TEST_UI_DIRECTORY": str(folder), "QUOTABAR_TEST_PRIVATE_PASTEBOARD": "1"}
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
                if route in ("dashboard", "accounts", "connect-claude", "connect-gemini", "remove"):
                    # A second frame from the untouched window measures e.g. caret blinking.
                    time.sleep(0.55)
                    controls.request("snapshot", "sheet" if sheet else "dashboard")
                    repeat = output / "repeat"
                    repeat.mkdir(exist_ok=True)
                    shutil.copyfile(folder / "frame.png", repeat / f"{route}-{theme}.png")
            except (ValueError, OSError) as error:
                observed[route] = {"error": str(error)}

        def copy_selection(key, label, scene):
            try:
                controls.request("selectText", label)
                capture(scene)
                observed[key + "SelectionStyle"] = controls.request("selectionStyle")
                controls.request("selectedText", expected={"text": label})
                controls.request("copySelection")
                observed[key + "Clipboard"] = controls.request("selectionClipboard")
            except (ValueError, TimeoutError) as error:
                observed[key + "CopyError"] = str(error)

        try:
            dashboard.wait_for(app.stdout, "ready")
            dashboard.send(app.stdin, {"state": {"_0": fixture}})
            for route in ("accounts", "dashboard", "settings"):
                show(route)
                if route == "accounts":
                    controls.request("size")
                capture(route)
                if route == "accounts":
                    account = fixture["settings"]["accounts"][0]
                    contact = deepcopy(fixture)
                    contact["snapshots"][1]["email"] = ACCOUNT_EMAIL
                    dashboard.send(app.stdin, {"state": {"_0": contact}})
                    # The new email row moves the name. Wait for the updated
                    # card before using its accessibility frame for the drag.
                    controls.request("present", ACCOUNT_EMAIL, {"found": True})
                    copy_selection("accountName", account["name"], "account-name-selected")
                    copy_selection("accountEmail", ACCOUNT_EMAIL, "account-email-selected")
                    contact["settings"]["accounts"][0]["sourcePath"] = SOURCE_PATH
                    dashboard.send(app.stdin, {"state": {"_0": contact}})
                    try:
                        controls.request("press", "연결한 로그인 파일")
                        # AppKit exposes the path before the disclosure animation finishes.
                        # Let the path settle under the pointer before starting the drag.
                        time.sleep(0.7)
                        copy_selection("sourcePath", SOURCE_PATH, "source-path-selected")
                    except (TimeoutError, ValueError) as error:
                        observed["sourcePathCopyError"] = str(error)
                    dashboard.send(app.stdin, {"state": {"_0": fixture}})
                if route == "dashboard":
                    # AccessKit publishes its first tree on the next UI frame.
                    try:
                        observed["appMark"] = controls.request("help", "QuotaBar 로고", {"found": True, "help": None})
                    except TimeoutError as error:
                        observed["appMark"] = {"error": str(error)}
                    message = BANNER_MESSAGE
                    dashboard.send(app.stdin, {"state": {"_0": {**fixture, "message": message}}})
                    capture("message")
                    copy_selection("message", message, "message-selected")
                    try:
                        controls.request("dismissBanner", message)
                        observed["messageDismissal"] = dashboard.wait_for(app.stdout, "message")
                        observed["messageAfterDismissal"] = controls.request("help", message, {"found": False, "help": None})
                    except (ValueError, TimeoutError) as error:
                        observed["messageError"] = str(error)
                    long_message = (message + " 연결한 로그인 파일의 위치와 계정의 모니터링 상태를 확인하세요. ") * 4
                    dashboard.send(app.stdin, {"state": {"_0": {**fixture, "message": long_message.strip()}}})
                    capture("message-wrapped")
                    dashboard.send(app.stdin, {"state": {"_0": fixture}})
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
            positions = []
            observed["settingsScroller"] = positions
            try:
                positions.append(controls.request("scrollerPosition", expected={"position": "top"})["position"])
                controls.request("pageSettings")
                positions.append(controls.request("scrollerPosition", expected={"position": "middle"})["position"])
                time.sleep(0.3)
                controls.request("snapshot", "dashboard")
                crops = [folder / "top.png", folder / "paged.png"]
                for source, crop in zip((output / f"settings-{theme}.png", folder / "frame.png"), crops):
                    subprocess.run(["sips", "--cropToHeightWidth", "320", "560", source, "--out", crop],
                                   stdout=subprocess.DEVNULL, check=True, timeout=8)
                difference = folder / "page-difference.json"
                subprocess.run([pixel_reader, "--pixels", *crops, difference, "--reference-ui"], check=True, timeout=12)
                # The centered crop contains the page body and excludes the track and its moving knob.
                observed["settingsPageChangedPixels"] = json.loads(difference.read_bytes())["changedPixels"]
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                observed["settingsScrollerError"] = str(error)
            for _ in range(3):
                controls.request("scrollContent")
                time.sleep(0.30)
            try:
                positions.append(controls.request("scrollerPosition", expected={"position": "bottom"})["position"])
            except (OSError, ValueError) as error:
                observed["settingsScrollerError"] = str(error)
            capture("settings-bottom")
            copy_selection("dataPath", DATA_PATH, "settings-path-selected")
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
            dashboard.send(app.stdin, {"state": {"_0": quota_status_fixture(fixture)}})
            show("dashboard")
            capture("quota-status")
            try:
                observed["quotaErrorLinkClick"] = controls.request("clickLink", "계정 관리에서 확인 →")
                observed["quotaErrorNavigation"] = commands_after_input(dashboard, app.stdout, "route")
            except (TimeoutError, ValueError) as error:
                observed["quotaErrorNavigation"] = {"error": str(error)}
            # Either possible route after clicking the link can transition to Settings.
            # Later scenes should still exercise the dashboard if this input fails.
            show("settings")
            for scene, state in activity_fixtures(fixture):
                if scene != "empty":
                    show("accounts")
                dashboard.send(app.stdin, {"state": {"_0": state}})
                show("dashboard")
                if scene != "empty":
                    time.sleep(0.35)
                    scroll_activity_to_bottom(controls)
                capture(scene)
                if scene != "empty":
                    observed[scene + "Scroll"] = controls.request("scrollerPosition")["position"]
                if scene == "activity" and "error" not in observed[scene]:
                    observed["activityTotals"] = controls.request("activityTotals")
                    today = datetime.fromtimestamp(state["demoReport"]["scannedAt"] + 978307200).date()
                    used = f"Claude {today - timedelta(days=1)}, 240 tokens"
                    observed["activityCells"] = {
                        "used": controls.request("help", used),
                        "unused": controls.request("help", f"Claude {today - timedelta(days=2)}, 0 tokens"),
                    }
                    try:
                        controls.request("press", used)
                        capture("activity-detail")
                        observed["activityDayOpened"] = controls.request("activityDetail")
                        controls.request("activityEscape")
                        capture("activity-escape")
                        observed["activityDayAfterEscape"] = controls.request("activityDetail")
                        show("accounts")
                        show("dashboard")
                        scroll_activity_to_bottom(controls)
                        capture("activity-return")
                        observed["activityDayReturnScroll"] = controls.request("scrollerPosition")["position"]
                        observed["activityDayAfterReturn"] = controls.request("activityDetail")
                    except (TimeoutError, ValueError) as error:
                        observed["activityDayError"] = str(error)
            observed["providerSwitches"] = {}
            observed["accessibleProviderSwitches"] = {}
            observed["providerSwitchEvents"] = {}
            for provider in PROVIDERS:
                state = deepcopy(fixture)
                account = state["settings"]["accounts"][0]
                account.update(provider=provider, showInMenuBar=True)
                label = f"{provider.title()}({account['name']}) 상단바에 표시"
                show("accounts")
                dashboard.send(app.stdin, {"state": {"_0": state}})
                show("dashboard")
                controls.request("present", label, {"found": True})
                details = observed["providerSwitchEvents"][provider] = {}
                for op, result in (("clickSwitch", "providerSwitches"), ("pressSwitch", "accessibleProviderSwitches")):
                    changes = observed[result][provider] = []
                    events = details[op] = []
                    for visible in (False, True):
                        if op == "clickSwitch":
                            capture(f"switch-{provider}-{'on' if account['showInMenuBar'] else 'off'}")
                        click = controls.request(op, label, value=not visible)
                        replies = commands_after_input(dashboard, app.stdout, "menuBarVisibility")
                        events.append({**click, "commands": replies})
                        if click["before"] == (not visible) and replies == [{"_0": account["id"], "_1": visible}]:
                            changes.append(visible)
                        account["showInMenuBar"] = visible
                        dashboard.send(app.stdin, {"state": {"_0": state}})
            # Exercise the header after capturing the unhovered reference scenes.
            refresh = observed["refreshButton"] = {}
            try:
                click = controls.request("clickButton", "새로고침")
                refresh["frame"] = click["frame"]
                refresh["click"] = commands_after_input(dashboard, app.stdout, "refreshAll")
                controls.request("press", "새로고침")
                refresh["press"] = commands_after_input(dashboard, app.stdout, "refreshAll")
                refresh["help"] = controls.request("help", "새로고침")
            except (TimeoutError, ValueError) as error:
                refresh["error"] = str(error)
            label = "계정 관리 →"
            manage = observed["manageAccountsButton"] = {}
            manage["frame"] = controls.request("clickButton", label)["frame"]
            manage["click"] = commands_after_input(dashboard, app.stdout, "route")
            show("dashboard")
            controls.request("press", label)
            manage["press"] = commands_after_input(dashboard, app.stdout, "route")
            show("settings")
            controls.request("scrollerPosition", expected={"position": "top"})
            controls.request("scrollContent")
            reopen = observed["repeatSettingsShow"] = {}
            reopen["before"] = controls.request("scrollerPosition", expected={"position": "bottom"})
            dashboard.send(app.stdin, {"show": {"route": "settings"}})
            reopen["routes"] = commands_after_input(dashboard, app.stdout, "route")
            reopen["after"] = controls.request("scrollerPosition")
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


def compare_window_placement(dashboard, work, original, library, fixture):
    results = {}
    for side, program in (("original", original), ("rust", dashboard.APP)):
        observed = results[side] = {}
        profile = work / f"placement-profile-{side}"
        profile.mkdir()
        for phase in ("fresh", "restored", "seeded"):
            folder = work / f"placement-{side}-{phase}"
            folder.mkdir()
            # Reopen the same isolated preferences after the first helper exits.
            # Also exercise the same saved frame from AppKit's argument domain.
            home = folder if phase == "seeded" else profile
            env = {**os.environ, **dashboard.fixture_environment(), "HOME": str(home), "CFFIXED_USER_HOME": str(home),
                   "DYLD_INSERT_LIBRARIES": str(library), "QUOTABAR_TEST_UI_DIRECTORY": str(folder)}
            args = [program, "--dashboard-helper", "--demo"]
            if phase == "seeded":
                args.extend(("-NSWindow Frame DashboardWindow", observed["moved"]["savedFrame"]))
            with subprocess.Popen(args, stdin=subprocess.PIPE, stdout=subprocess.PIPE, bufsize=0, env=env) as app:
                controls = Controls(folder, app)
                try:
                    dashboard.wait_for(app.stdout, "ready")
                    dashboard.send(app.stdin, {"state": {"_0": fixture}})
                    dashboard.send(app.stdin, {"show": {"route": "dashboard"}})
                    # Both helpers start on Dashboard. Opening that route makes the
                    # window visible without publishing a navigation change.
                    observed[phase] = controls.request("windowPlacement")
                    if phase == "fresh":
                        # The Dock's visible frame can move between launches. Drive both
                        # windows to the very same coordinates before reopening them.
                        target = results["original"].get("moved", {}).get("frame")
                        observed["moved"] = controls.request("moveDashboard", value=target)
                except (OSError, ValueError, RuntimeError, TimeoutError, EOFError, subprocess.SubprocessError) as error:
                    observed["error"] = str(error)
                    break
                finally:
                    if app.poll() is None:
                        try:
                            dashboard.send(app.stdin, {"close": {}})
                            app.wait(timeout=4)
                        except (OSError, subprocess.TimeoutExpired):
                            app.kill()
    matched = results["original"] == results["rust"] and all(
        "error" not in observed
        and all(observed[phase]["identifier"] == "dashboard" and observed[phase]["autosave"] == "DashboardWindow"
                for phase in ("fresh", "restored", "seeded"))
        and observed["fresh"]["frame"] != observed["moved"]["frame"] == observed["restored"]["frame"] == observed["seeded"]["frame"]
        for observed in results.values())
    results["identical"] = matched
    (dashboard.REPORT / "window-placement.json").write_text(json.dumps(results, indent=2))
    return matched


def compare_windows(dashboard, work, original, library):
    dest = dashboard.REPORT / "windows"
    results = {"original": {}, "rust": {}, "comparisons": {}, "repeatability": {}}
    fixtures = {theme: json.loads((dashboard.REPORT / f"fixture-{theme}.json").read_text())
                for theme in dashboard.THEMES}
    for side, program in (("original", original), ("rust", dashboard.APP)):
        output = dest / side
        output.mkdir(parents=True, exist_ok=True)
        for theme, fixture in fixtures.items():
            results[side][theme] = capture_windows(dashboard, program, work / f"windows-{side}-{theme}",
                                                  library, fixture, output, theme, original)
    for theme in dashboard.THEMES:
        for route in (*dashboard.ROUTES, "remove", "message", "message-selected", "message-wrapped", "account-name-selected", "account-email-selected", "popover-sample", "popover-empty", "popover-overflow",
                      "source-path-selected", "settings-path-selected", "settings-bottom", "licenses-notice",
                      "licenses-apache", "empty", "quota-status", "activity-empty", "activity", "activity-detail",
                      "activity-escape", "activity-return",
                      *(f"switch-{provider}-{state}" for provider in PROVIDERS for state in ("on", "off"))):
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
    frames = {
        side: [event["frame"] for theme in dashboard.THEMES for provider in PROVIDERS
               for op in ("clickSwitch", "pressSwitch")
               for event in results[side][theme].get("providerSwitchEvents", {}).get(provider, {}).get(op, [])]
        for side in ("original", "rust")
    }
    frames_match = len(frames["original"]) == len(dashboard.THEMES) * len(PROVIDERS) * 4 and frames["original"] == frames["rust"]
    results["providerSwitchFramesIdentical"] = frames_match
    refresh_matches = True
    manage_matches = True
    for theme in dashboard.THEMES:
        expected = results["original"][theme].get("refreshButton", {})
        actual = results["rust"][theme].get("refreshButton", {})
        refresh_matches &= (expected == actual and bool(expected.get("frame"))
                            and expected.get("click") == [{}] and expected.get("press") == [{}]
                            and expected.get("help") == {"found": True, "help": "새로고침 (⌘R)"})
        expected = results["original"][theme].get("manageAccountsButton", {})
        actual = results["rust"][theme].get("manageAccountsButton", {})
        manage_matches &= (expected == actual and bool(expected.get("frame"))
                           and expected.get("click") == expected.get("press") == [{"_0": "accounts"}])
    results["refreshButtonsIdentical"] = refresh_matches
    results["manageAccountsButtonsIdentical"] = manage_matches
    quota_error_matches = all(results[side][theme].get("quotaErrorNavigation") == [{"_0": "accounts"}]
                              for side in ("original", "rust") for theme in dashboard.THEMES)
    results["quotaErrorNavigationIdentical"] = quota_error_matches
    expected_reopen = {"before": {"position": "bottom"}, "routes": [], "after": {"position": "bottom"}}
    reopen_matches = all(results[side][theme].get("repeatSettingsShow") == expected_reopen
                         for side in ("original", "rust") for theme in dashboard.THEMES)
    results["repeatSettingsShowIdentical"] = reopen_matches
    app_mark_matches = all(results[side][theme].get("appMark") == {"found": True, "help": None}
                           for side in ("original", "rust") for theme in dashboard.THEMES)
    message_matches = all(results[side][theme].get("messageDismissal") == {}
                          and results[side][theme].get("messageAfterDismissal") == {"found": False, "help": None}
                          and results[side][theme].get("messageClipboard") == {"text": BANNER_MESSAGE}
                          for side in ("original", "rust") for theme in dashboard.THEMES)
    copied_accounts = all(
        results[side][theme].get("accountNameClipboard") == {"text": fixture["settings"]["accounts"][0]["name"]}
        and results[side][theme].get("accountEmailClipboard") == {"text": ACCOUNT_EMAIL}
        and results[side][theme].get("sourcePathClipboard") == {"text": SOURCE_PATH}
        and results[side][theme].get("dataPathClipboard") == {"text": DATA_PATH}
        for side in ("original", "rust") for theme, fixture in fixtures.items())
    documents_match = all(results[side][theme].get("licenseDocuments") == ["NOTICE", "LICENSE", "NOTICE", "closed"]
                          and results[side][theme].get("licenseEscapeClosed") is True
                          for side in ("original", "rust") for theme in dashboard.THEMES)
    scroll_matches = all(results[side][theme].get("settingsScroller") == ["top", "middle", "bottom"]
                         and results[side][theme].get("settingsPageChangedPixels", 0) > 1000
                         for side in ("original", "rust") for theme in dashboard.THEMES)
    activity_matches = all(results[side][theme].get("activityScroll") == "bottom"
                           and results[side][theme].get("activity-emptyScroll") == "bottom"
                           and results[side][theme].get("activityTotals") == {"claude": True, "codex": True}
                           and results[side][theme].get("activityCells") == {
                               "used": {"found": True, "help": "날짜별 모델 사용량 보기"},
                               "unused": {"found": False, "help": None},
                           }
                           for side in ("original", "rust") for theme in dashboard.THEMES)
    switch_matches = all(results[side][theme].get(kind) == {p: [False, True] for p in PROVIDERS}
                         for side in ("original", "rust") for theme in dashboard.THEMES
                         for kind in ("providerSwitches", "accessibleProviderSwitches"))
    # SwiftUI routes Escape through the current first responder. Activating a
    # date via AX can leave the dashboard's keyboard focus elsewhere.
    escape_states = {side: [results[side][theme].get("activityDayAfterEscape", {}).get("model")
                            for theme in dashboard.THEMES] for side in ("original", "rust")}
    day_detail_matches = (all(isinstance(v, bool) for v in escape_states["original"])
                          and escape_states["original"] == escape_states["rust"]
                          and all(results[side][theme].get("activityDayOpened", {}).get("model") is True
                             and results[side][theme].get("activityDayReturnScroll") == "bottom"
                             and results[side][theme].get("activityDayAfterReturn", {}).get("model") is False
                             for side in ("original", "rust") for theme in dashboard.THEMES))
    results["activityDetailIdentical"] = day_detail_matches
    (dest / "comparison.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
    return (app_mark_matches and message_matches and copied_accounts and documents_match and scroll_matches
            and activity_matches and switch_matches and frames_match and refresh_matches and manage_matches and reopen_matches
            and quota_error_matches and day_detail_matches
            and all(row["identical"] for row in results["comparisons"].values()))


def compare(dashboard, work, original, fixture, library):
    fixture = {**fixture, "settings": {**fixture["settings"], "scanLocalLogs": False}}
    results = {}
    for side, program in (("original", original), ("rust", dashboard.APP)):
        record = {"observed": [], "help": {}}
        results[side] = record
        try:
            exercise(dashboard, program, work / side, library, deepcopy(fixture), record["observed"], record["help"])
        except (OSError, ValueError, KeyError, RuntimeError, EOFError, subprocess.SubprocessError) as error:
            record["error"] = str(error)
        print(f"{side}: {len(record['observed'])} dashboard actions; {record.get('error', 'complete')}", flush=True)
    completed = all("error" not in results[side] for side in ("original", "rust"))
    results["identical"] = completed and results["original"]["observed"] == results["rust"]["observed"]
    # Keep the full traffic above, including duplicate switch notifications.
    # A user action completes only after its expected command and resulting state are verified.
    def outcomes(side):
        return [action for action in results[side]["observed"] if "repeatedPreference" not in action]
    results["userActionsIdentical"] = completed and outcomes("original") == outcomes("rust")
    expected_help = {"tray": "상단바 요약과 팝업에 표시합니다. 꺼도 사용량 조회는 계속됩니다.",
                     "monitoring": "끄면 이 계정의 조회를 일시 정지합니다."}
    results["switchHelpIdentical"] = all(results[side]["help"].get(key) == {"help": value, "found": True}
                                        for side in ("original", "rust") for key, value in expected_help.items())
    results["connectionErrorCopies"] = all(
        results[side]["help"].get("connectionErrorClipboard") == {"text": FILE_ERROR_TEXT}
        for side in ("original", "rust"))

    def fields_match(name, count):
        def ordered(side):
            fields = results[side]["help"].get(name, {}).get("fields", [])
            return sorted(fields, key=lambda field: field["placeholder"])

        original = ordered("original")
        return len(original) == count and original == ordered("rust")

    results["connectionFieldsIdentical"] = fields_match("connectionFields", 2)
    results["searchFieldIdentical"] = fields_match("searchFields", 1)
    (dashboard.REPORT / "actions.json").write_text(json.dumps(results, ensure_ascii=False, indent=2))
    return (results["identical"] and results["switchHelpIdentical"] and results["connectionErrorCopies"]
            and results["connectionFieldsIdentical"] and results["searchFieldIdentical"])
