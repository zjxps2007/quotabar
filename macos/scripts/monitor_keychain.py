"""Launch the packaged monitor twice with disabled accounts and a disposable legacy macOS keychain."""
import json
import os
from pathlib import Path
import secrets
import stat
import subprocess
from uuid import UUID

SERVICE = "com.aiusage.lite.credentials"
IDS = [str(UUID(int=0xCA00 + i)).upper() for i in range(5)]


def dashboard(wire, incoming, outgoing):
    wire["send"](outgoing, {"ready": {}})
    state = wire["wait_for"](incoming, "state")["_0"]
    settings = state["settings"]
    assert settings["credentialStorageVersion"] == 1 and [a["id"] for a in settings["accounts"]] == IDS
    assert not state["loading"] and not state.get("message")
    assert "fixture-secret" not in json.dumps(state)
    Path(os.environ["QUOTABAR_TEST_MIGRATION_STATE"]).write_text(json.dumps(settings))
    wire["send"](outgoing, {"quit": {}})


def login(workspace, token):
    return json.dumps({"tokens": {"access_token": "fixture-secret-" + token, "account_id": workspace},
                       "user_id": "fixture-user"})


def compare(bundle, directory, inherited):
    home = (directory / "migration-home").resolve()
    root = home / "Library/Application Support/AIUsage Lite"
    vault = root / "Credentials"
    vault.mkdir(mode=0o700, parents=True)
    path = home / "fixture.keychain-db"
    trace = home / "legacy-actions.jsonl"
    trace.touch()
    account_rows = []
    for i, uuid in enumerate(IDS):
        cli = home / f"source-{i}.json"
        cli.write_text(login(str(i), "cli"))
        account_rows.append(dict(id=uuid, name=f"Migration {i}", provider="claude" if i == 4 else "codex",
                                 sourcePath=str(cli), identity=f"codex:account:{i}:user:fixture-user",
                                 appManagedLogin=i in (0, 2), enabled=False, createdAt=100 + i))
    settings = dict(accounts=account_rows, refreshSeconds=300, autoRefresh=False, automaticUpdates=False, scanLocalLogs=False)
    settings_file = root / "settings.json"
    settings_file.write_text(json.dumps(settings))
    local = vault / (IDS[3] + ".json")
    local.write_text(login("3", "newer-vault"))
    local.chmod(0o600)
    env = {**inherited, "HOME": str(home), "CFFIXED_USER_HOME": str(home), "QUOTABAR_TEST_MONITOR_HOME": str(home),
           "QUOTABAR_TEST_KEYCHAIN": str(path), "CODEX_HOME": str(home / "Codex"), "CLAUDE_CONFIG_DIR": str(home / "Claude")}

    def security(*args, check=True):
        result = subprocess.run(["/usr/bin/security", *args, str(path)], capture_output=True, timeout=12)
        if check and result.returncode:
            raise RuntimeError(f"Temporary keychain {args[0]} returned {result.returncode}: "
                               + result.stderr.decode(errors="replace").strip())
        return result

    def add(i, value):
        security("add-generic-password", "-A", "-s", SERVICE, "-a", IDS[i], "-w", value)

    def launch(attempt):
        state_file = home / f"{attempt}-state.json"
        subprocess.run([bundle / "Contents/MacOS/QuotaBar"], check=True, timeout=20,
                       env={**env, "QUOTABAR_TEST_MIGRATION_STATE": str(state_file)})
        assert json.loads(state_file.read_bytes()) == {**settings, "credentialStorageVersion": 1}
        assert json.loads(settings_file.read_bytes()) == json.loads(state_file.read_bytes())

    password = secrets.token_hex(24)
    security("create-keychain", "-p", password)
    try:
        security("unlock-keychain", "-p", password)
        for i, workspace in enumerate(("0", "wrong", "wrong", "3", "unused")):
            add(i, login(workspace, "keychain"))
        launch("first")
        records = [json.loads(line) for line in trace.read_text().splitlines()]
        reads = [r["id"] for r in records if r["operation"] == "read"]
        deletes = [r for r in records if r["operation"] == "delete"]
        assert reads == IDS[:3] and [r["id"] for r in deletes] == [IDS[i] for i in (0, 1, 3)], records
        assert all(r["quiet"] for r in records) and all(r["marker"] == 1 and r["vault"] for r in deletes), records
        expected = {0: login("0", "keychain"), 1: login("1", "cli"), 3: login("3", "newer-vault")}
        for i, uuid in enumerate(IDS):
            entry = vault / (uuid + ".json")
            assert entry.exists() == (i in expected)
            if entry.exists():
                assert entry.read_text() == expected[i] and stat.S_IMODE(entry.stat().st_mode) == 0o600
            result = security("find-generic-password", "-s", SERVICE, "-a", uuid, check=False)
            assert result.returncode == (44 if i in expected else 0), (i, result.returncode)
        add(0, login("0", "must-remain-on-restart"))
        before = trace.read_bytes()
        launch("restart")
        assert trace.read_bytes() == before
        security("find-generic-password", "-s", SERVICE, "-a", IDS[0])
        return {"matchingKeychainCopied": True, "importedCliFallback": True, "noManagedCliFallback": True,
                "newerVaultPreserved": True, "claudeSkipped": True, "nonInteractive": True,
                "deletionAfterCommittedSettings": True, "restartDidNotReadOrDelete": True}
    finally:
        security("delete-keychain")
