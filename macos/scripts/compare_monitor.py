"""Exercise both packaged, non-demo monitors through their normal dashboard pipe."""
import hashlib
import json
import os
from pathlib import Path
import plistlib
import runpy
import shutil
import subprocess
import sys
import time
import traceback
from uuid import UUID
import monitor_providers
import monitor_update_download

HERE = Path(__file__).resolve().parent
IDS = [str(UUID(int=i)).upper() for i in range(1, 7)]
MISSING = "저장된 로그인이 없습니다. 계정을 다시 연결하세요."


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")


def post_statusline(program, sample, generation, used, env):
    session = sample.stem.removeprefix("snapshot-")
    data = {"session_id": session, "rate_limits": {"five_hour": {
        "used_percentage": used, "resets_at": int(time.time()) + 3600}}}
    subprocess.run([program, "--claude-statusline", sample.parent.name, generation, sample.parents[1]],
                   input=json.dumps(data).encode(), env=env, stdout=subprocess.DEVNULL, check=True, timeout=10)
    assert json.loads(sample.read_bytes())["fiveHour"]["usedPercent"] == used


def fixture(home, config, program, env):
    root = home / "Library/Application Support/AIUsage Lite"
    identity = hashlib.sha256(str(config).encode()).hexdigest()
    profiles = root / "ClaudeProfiles" / identity
    profiles.mkdir(parents=True)
    now = int(time.time()) - 978307200
    session, generation = str(UUID(int=21)), str(UUID(int=22))
    profile = dict(id=identity, configDirectory=str(config), name="Fixture", generation=generation,
                   installedCommand="/usr/bin/true", subscriptionType="max")
    write_json(profiles / "profile.json", profile)
    write_json(config / "settings.json", {"statusLine": {"type": "command", "command": profile["installedCommand"]}})
    sample = profiles / f"snapshot-{session}.json"
    post_statusline(program, sample, generation, 37, env)
    accounts = []
    for index, account in enumerate(IDS):
        provider = "claude" if index == 0 else ("codex", "gemini")[index % 2]
        accounts.append(dict(id=account, name=f"Fixture {index}", provider=provider,
                             sourcePath=str(config if not index else home / f"unavailable-{index}.json"),
                             identity=identity if not index else f"fixture-{index}", createdAt=now, enabled=index < 5))
    write_json(root / "settings.json", dict(accounts=accounts, refreshSeconds=300, autoRefresh=False,
               automaticUpdates=False, scanLocalLogs=False, credentialStorageVersion=1, showDockIcon=False))
    return root, sample


def dashboard_probe():
    # The test dashboard is a client of the monitor. Only its two private pipes
    # carry app commands and state; the monitor executable itself is unmodified.
    wire = runpy.run_path(str(HERE / "check-dashboard.py"))
    incoming, outgoing = sys.stdin.buffer, sys.stdout.buffer
    if stage := os.environ.get("QUOTABAR_TEST_HOST_STAGES"):
        directory = Path(stage)
        try:
            return host_probe(wire, incoming, outgoing, directory)
        except Exception:
            (directory / "error").write_text(traceback.format_exc())
            raise
    output = Path(os.environ["QUOTABAR_TEST_REFRESH_RESULT"])
    sample = Path(os.environ["QUOTABAR_TEST_SAMPLE"])
    checkpoints = []
    updates = []
    providers = []
    transfers = []

    def command(name, *values):
        wire["send"](outgoing, {name: {f"_{i}": v for i, v in enumerate(values)}})

    def toggle_popover(opening):
        command("showPopover")
        name = "popover-opened" if opening else "popover-closed"
        log = output.parent / "diagnostics.jsonl"
        deadline = time.monotonic() + 6
        while time.monotonic() < deadline:
            if log.exists():
                for line in log.read_bytes().splitlines(keepends=True):
                    if not line.endswith(b"\n"):
                        continue
                    row = json.loads(line)
                    if row["event"] == name:
                        observed = {key: row[key] for key in ("event", "dashboardRetained", "popoverRetained", "popoverVisible")}
                        assert observed == dict(event=name, dashboardRetained=True, popoverRetained=opening, popoverVisible=opening), observed
                        return observed
            time.sleep(0.05)
        raise TimeoutError(name)

    def set_usage(used):
        saved = json.loads(sample.read_bytes())
        post_statusline(os.environ["QUOTABAR_TEST_STATUSLINE"], sample, saved["generation"], used, os.environ)

    def check_update(name, status, tag, phase, version=None, message=None):
        home = output.parent
        response = {"tag_name": tag, "draft": False, "prerelease": False, "assets": [],
                    "html_url": f"https://github.com/zjxps2007/quotabar/releases/tag/{tag}"} if tag else {}
        tmp = home / "release-response.tmp"
        write_json(tmp, {"status": status, "body": response})
        tmp.replace(home / "release-response.json")
        command("checkAppUpdate")
        deadline = time.monotonic() + 8
        last = None
        while time.monotonic() < deadline:
            try:
                event = wire["receive"](incoming, deadline)
            except TimeoutError:
                break
            if "state" not in event:
                continue
            last = event["state"]["_0"].get("appUpdate", {})
            if (last.get("phase"), last.get("version"), last.get("message")) != (phase, version, message):
                continue
            assert last.get("checkedAt") is not None, last
            updates.append({"step": name, "phase": phase, "version": "installed" if phase == "current" else version,
                            "message": message, "http": status})
            return
        raise AssertionError((name, last))

    def expect(name, remaining, enabled, failed, members=IDS, marker=None):
        deadline = time.monotonic() + 8
        observed = None
        while time.monotonic() < deadline:
            try:
                event = wire["receive"](incoming, deadline)
            except TimeoutError:
                break
            if "state" not in event:
                continue
            state = event["state"]["_0"]
            rows = state["settings"]["accounts"]
            snapshots, errors = (dict(zip(state[key][::2], state[key][1::2])) for key in ("snapshots", "errors"))
            claude = snapshots.get(IDS[0], {})
            windows = claude.get("windows", [])
            observed = dict(accounts=[a["id"] for a in rows], enabled=[a["id"] for a in rows if a["enabled"]],
                            remaining=[w["remaining"] for w in windows], state=claude.get("state"),
                            errors=errors, loading=state["loading"], message=state.get("message"))
            if (observed["accounts"] == members and observed["enabled"] == enabled and
                    observed["remaining"] == [remaining] and observed["state"] == "snapshot" and
                    errors == dict.fromkeys(failed, MISSING) and not state["loading"] and
                    set(snapshots) == {IDS[0]} and (marker is None or observed["message"] == marker)):
                checkpoints.append({"step": name, **observed})
                return
        raise AssertionError((name, observed))

    try:
        command("ready")
        expect("startup", 63, IDS[:5], IDS[1:5])
        opened = toggle_popover(True)
        set_usage(75)
        command("refresh", IDS[0])
        expect("manual refresh", 25, IDS[:5], IDS[1:5])
        closed = toggle_popover(False)
        command("toggle", IDS[0])
        expect("disable", 25, IDS[1:5], IDS[1:5])
        set_usage(12)
        command("refreshAccounts")
        command("message", "refresh barrier")
        expect("refresh while disabled", 25, IDS[1:5], IDS[1:5], marker="refresh barrier")
        command("toggle", IDS[0])
        expect("enable and refresh", 88, IDS[:5], IDS[1:5])
        command("toggle", IDS[5])
        expect("enable missing login", 88, IDS, IDS[1:])
        command("remove", IDS[5])
        expect("remove and clear error", 88, IDS[:5], IDS[1:5], members=IDS[:5])
        with (Path(os.environ["QUOTABAR_TEST_STATUSLINE"]).parents[1] / "Info.plist").open("rb") as info:
            installed = plistlib.load(info)["CFBundleShortVersionString"]
        offered = "2147483646.0.0"
        check_update("no published release", 404, None, "idle", message="공개된 정식 릴리즈가 없습니다.")
        check_update("installed version is current", 200, "v0.0.1", "current", installed)
        check_update("manual check with automatic updates off", 200, "v" + offered, "available", offered)
        command("automaticUpdates", True)
        check_update("automatic update without a platform asset", 200, "v" + offered, "available", offered,
                     "이 Mac에 맞는 업데이트 파일이 아직 없습니다.")
        command("automaticUpdates", False)
        check_update("rate-limited feed", 429, None, "failed", message="업데이트 확인 요청이 제한되었습니다. 잠시 후 다시 시도하세요.")
        check_update("retry after rate limit", 200, "v" + offered, "available", offered)
        requests = (output.parent / "release-requests.log").read_text().splitlines()
        assert requests == [str(s["http"]) for s in updates], requests
        traffic = monitor_providers.exercise(wire, incoming, outgoing, output.parent, providers)
        monitor_update_download.exercise(wire, incoming, command, output.parent, transfers)
        write_json(output, {"checkpoints": checkpoints, "updates": updates, "providers": providers, "popover": [opened, closed],
                            "providerRequests": traffic, "updateDownloads": transfers})
    except Exception:
        write_json(output, {"checkpoints": checkpoints, "updates": updates, "providers": providers,
                            "updateDownloads": transfers, "error": traceback.format_exc()})
    finally:
        command("quit")
    try:
        wire["wait_for"](incoming, "close")
    except EOFError:
        pass


def host_probe(wire, incoming, outgoing, directory):
    stage = (directory / "stage").read_text()
    if stage == "timeout":
        began = time.monotonic()
        first = wire["receive"](incoming, began + 20)
        assert first == {"close": {}} and time.monotonic() - began >= 8, first
        write_json(directory / stage, {"firstFrame": "close"})
        return
    wire["send"](outgoing, {"ready": {}})
    deadline = time.monotonic() + 10
    first = wire["receive"](incoming, deadline)
    state = first["state"]["_0"]
    route = wire["wait_for"](incoming, "show")["route"]
    previous = state.get("message")
    if stage == "reopen":
        assert previous == "Dashboard가 응답하지 않습니다. 다시 열어 주세요." and route == "dashboard", (previous, route)
        wire["send"](outgoing, {"visibility": {"_0": True}})
        wire["send"](outgoing, {"route": {"_0": "settings"}})
        wire["send"](outgoing, {"message": {"_0": "host state barrier"}})
        while wire["wait_for"](incoming, "state")["_0"].get("message") != "host state barrier":
            if time.monotonic() > deadline:
                raise TimeoutError("The store did not publish the route change")
        wire["send"](outgoing, {"visibility": {"_0": False}})
    else:
        assert stage == "restore" and previous == "host state barrier" and route == "settings", (stage, previous, route)
    write_json(directory / stage, {"firstFrame": "state", "showRoute": route, "message": previous})
    if stage == "restore":
        wire["send"](outgoing, {"quit": {}})


def host_recovery(bundle, home, env):
    directory = home / "host-stages"
    directory.mkdir()
    marker = directory / "stage"
    marker.write_text("timeout")
    log = directory / "diagnostics.jsonl"
    with subprocess.Popen([bundle / "Contents/MacOS/QuotaBar", "--demo", "--diagnostics", log],
                          env={**env, "QUOTABAR_TEST_HOST_STAGES": str(directory)}) as monitor:
        try:
            for count, stage in enumerate(("reopen", "restore"), 1):
                deadline = time.monotonic() + 27
                while time.monotonic() < deadline:
                    if (directory / "error").exists():
                        raise ValueError((directory / "error").read_text())
                    records = [] if not log.exists() else [json.loads(line) for line in log.read_text().splitlines(keepends=True) if line.endswith("\n")]
                    if sum(row["event"] == "dashboard-process-exited" for row in records) == count:
                        break
                    if monitor.poll() is not None:
                        raise AssertionError(f"Monitor exited before {stage}")
                    time.sleep(0.1)
                else:
                    raise TimeoutError(f"Timed out before {stage}: {[r['event'] for r in records]}")
                marker.write_text(stage)
                # Launch Services reopens the registered app just like clicking its Dock icon.
                subprocess.run(["/usr/bin/open", "-a", str(bundle)], timeout=8, check=True)
            deadline = time.monotonic() + 20
            while monitor.poll() is None and time.monotonic() < deadline:
                if (directory / "error").exists():
                    raise ValueError((directory / "error").read_text())
                time.sleep(0.1)
            assert monitor.poll() == 0, "The reopened monitor did not quit"
            return {stage: json.loads((directory / stage).read_text()) for stage in ("timeout", "reopen", "restore")}
        finally:
            if monitor.poll() is None:
                monitor.kill()


def compare(dashboard, work, reference):
    library, probe = work / "fixture-home.dylib", work / "foundation-home"
    subprocess.run(["xcrun", "clang", "-fobjc-arc", "-dynamiclib", "-framework", "Foundation",
                    HERE / "monitor-fixture-home.m", "-o", library], check=True)
    source = work / "home.swift"
    source.write_text("""import Foundation
print(NSHomeDirectory())
print(FileManager.default.homeDirectoryForCurrentUser.path)
print(URL(fileURLWithPath: CommandLine.arguments[1]).standardizedFileURL.resolvingSymlinksInPath().path)
""")
    subprocess.run(["swiftc", source, "-o", probe], check=True)
    signed_update = monitor_update_download.signed_fixture(work)
    results = {}
    for side, executable in (("original", reference), ("rust", dashboard.MONITOR)):
        home = (work / f"{side}-monitor-home").resolve()
        home.mkdir()
        (home / "release-requests.log").touch()
        cli = home / "Claude fixture"
        cli.mkdir()
        result, diagnostics = home / "result.json", home / "diagnostics.jsonl"
        env = {**{k: v for k, v in os.environ.items() if not k.startswith(("ANTHROPIC_", "CLAUDE_CODE_"))},
               "HOME": str(home), "CFFIXED_USER_HOME": str(home),
               "CODEX_HOME": str(home / "Codex"), "CLAUDE_CONFIG_DIR": str(cli),
               "DYLD_INSERT_LIBRARIES": str(library), "QUOTABAR_TEST_MONITOR_HOME": str(home),
               "QUOTABAR_TEST_REFRESH_RESULT": str(result), "QUOTABAR_TEST_STATUSLINE": str(executable)}
        try:
            paths = subprocess.check_output([probe, cli], env=env, text=True, timeout=10).splitlines()
            if len(paths) != 3 or paths[:2] != [str(home), str(home)] or not cli.samefile(paths[2]):
                raise ValueError(f"Fixture path verification failed: {paths}")
            # Both monitors hash Foundation's standardized path. On macOS its
            # spelling of /var or /tmp can differ from Python's realpath.
            print(f"{side} Claude fixture: Foundation path differs from realpath = {str(cli) != paths[2]}", flush=True)
            root, sample = fixture(home, Path(paths[2]), executable, env)
            env["QUOTABAR_TEST_SAMPLE"] = str(sample)
            bundle = home / "QuotaBar.app"
            shutil.copytree(executable.parents[2], bundle, symlinks=True)
            monitor_update_download.install_fixture(signed_update, home, bundle)
            client = bundle / "Contents/Helpers/QuotaBar Dashboard.app/Contents/MacOS/QuotaBar"
            client.unlink()
            client.write_text(f"#!{sys.executable}\nimport sys\nsys.path.insert(0, {str(HERE)!r})\n"
                              "from compare_monitor import dashboard_probe\ndashboard_probe()\n")
            client.chmod(0o755)
            subprocess.run([bundle / "Contents/MacOS/QuotaBar", "--diagnostics", diagnostics], env=env, timeout=65, check=True)
            observed = json.loads(result.read_text())
            if "error" in observed:
                raise ValueError(observed["error"])
            records = [json.loads(line) for line in diagnostics.read_text().splitlines()]
            startup = next(row for row in records if row["event"] == "ui-launched")
            assert not startup["demo"] and (startup["runningRequests"], startup["queuedRequests"]) == (3, 2), startup
            saved = json.loads((root / "settings.json").read_text())
            assert (root / "instance.lock").is_file() and [a["id"].upper() for a in saved["accounts"]] == IDS[:5]
            results[side] = {**observed, "runningAtLaunch": 3, "queuedAtLaunch": 2, "persistedAccounts": IDS[:5],
                             "dashboardRecovery": host_recovery(bundle, home, env)}
        except (OSError, ValueError, AssertionError, StopIteration, subprocess.SubprocessError) as exc:
            details = json.loads(result.read_text()) if result.exists() else {}
            requests = (home / "release-requests.log").read_text().splitlines()
            results[side] = {**details, "releaseResponses": requests, "error": str(exc)}
    matched = "error" not in results["original"] and results["original"] == results["rust"]
    write_json(dashboard.REPORT / "monitor-refresh.json", {"matched": matched, **results})
    print("Packaged non-demo account refresh and update checks:", "PASS" if matched else results, flush=True)
    return matched
