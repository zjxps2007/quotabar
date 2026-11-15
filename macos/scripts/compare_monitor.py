"""Exercise both packaged, non-demo monitors through their normal dashboard pipe."""
import hashlib
import json
import os
from pathlib import Path
import runpy
import shutil
import subprocess
import sys
import time
import traceback
from uuid import UUID

HERE = Path(__file__).resolve().parent
IDS = [str(UUID(int=i)).upper() for i in range(1, 7)]
MISSING = "저장된 로그인이 없습니다. 계정을 다시 연결하세요."


def write_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False), encoding="utf-8")


def fixture(home):
    root = home / "Library/Application Support/AIUsage Lite"
    config = home / "Claude fixture"
    config.mkdir(parents=True)
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
    write_json(sample, dict(schemaVersion=1, profileID=identity, generation=generation, sessionID=session,
                            receivedAt=now, observedAt=now, fiveHour=dict(usedPercent=37, resetAt=now + 3600)))
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
    output = Path(os.environ["QUOTABAR_TEST_REFRESH_RESULT"])
    sample = Path(os.environ["QUOTABAR_TEST_SAMPLE"])
    checkpoints = []

    def command(name, *values):
        wire["send"](outgoing, {name: {f"_{i}": v for i, v in enumerate(values)}})

    def set_usage(used):
        saved = json.loads(sample.read_text())
        saved["fiveHour"]["usedPercent"] = used
        write_json(sample, saved)

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
        set_usage(75)
        command("refresh", IDS[0])
        expect("manual refresh", 25, IDS[:5], IDS[1:5])
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
        write_json(output, {"checkpoints": checkpoints})
    except Exception:
        write_json(output, {"checkpoints": checkpoints, "error": traceback.format_exc()})
    finally:
        command("quit")
    try:
        wire["wait_for"](incoming, "close")
    except EOFError:
        pass


def compare(dashboard, work, reference):
    library, probe = work / "fixture-home.dylib", work / "foundation-home"
    subprocess.run(["xcrun", "clang", "-fobjc-arc", "-dynamiclib", "-framework", "Foundation",
                    HERE / "monitor-fixture-home.m", "-o", library], check=True)
    source = work / "home.swift"
    source.write_text("import Foundation\nprint(NSHomeDirectory())\nprint(FileManager.default.homeDirectoryForCurrentUser.path)\n")
    subprocess.run(["swiftc", source, "-o", probe], check=True)
    results = {}
    for side, executable in (("original", reference), ("rust", dashboard.MONITOR)):
        home = (work / f"{side}-monitor-home").resolve()
        home.mkdir()
        root, sample = fixture(home)
        result, diagnostics = home / "result.json", home / "diagnostics.jsonl"
        env = {**os.environ, "DYLD_INSERT_LIBRARIES": str(library), "QUOTABAR_TEST_MONITOR_HOME": str(home),
               "QUOTABAR_TEST_REFRESH_RESULT": str(result), "QUOTABAR_TEST_SAMPLE": str(sample)}
        try:
            homes = subprocess.check_output([probe], env=env, text=True, timeout=10).splitlines()
            if homes != [str(home), str(home)]:
                raise ValueError(f"Fixture home verification failed: {homes}")
            bundle = home / "QuotaBar.app"
            shutil.copytree(executable.parents[2], bundle, symlinks=True)
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
            results[side] = {**observed, "runningAtLaunch": 3, "queuedAtLaunch": 2, "persistedAccounts": IDS[:5]}
        except (OSError, ValueError, AssertionError, StopIteration, subprocess.SubprocessError) as exc:
            details = json.loads(result.read_text()) if result.exists() else {}
            results[side] = {**details, "error": str(exc)}
    matched = "error" not in results["original"] and results["original"] == results["rust"]
    write_json(dashboard.REPORT / "monitor-refresh.json", {"matched": matched, **results})
    print("Packaged non-demo account refresh:", "PASS" if matched else results, flush=True)
    return matched
