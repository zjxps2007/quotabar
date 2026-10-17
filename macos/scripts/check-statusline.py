#!/usr/bin/env python3
"""Exercise the packaged helper with disposable fixtures, without launching UI."""
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid

app = Path(__file__).resolve().parents[2] / "artifacts/QuotaBar.app/Contents/MacOS/QuotaBar"
with tempfile.TemporaryDirectory(prefix="quotabar-statusline-", dir="/tmp") as temporary:
    root = Path(temporary)
    config = root / "profile with spaces"
    config.mkdir()
    identity = hashlib.sha256(str(config).encode()).hexdigest()
    profiles = root / "snapshots"
    folder = profiles / identity
    folder.mkdir(parents=True)
    generation = str(uuid.uuid4())
    profile = {"id": identity, "configDirectory": str(config), "name": "Smoke fixture", "generation": generation,
               "originalStatusLine": base64.b64encode(json.dumps({"type": "command", "command": "/bin/cat"}).encode()).decode()}
    (folder / "profile.json").write_text(json.dumps(profile))
    payload = json.dumps({"session_id": str(uuid.uuid4()), "rate_limits": {"five_hour": {
        "used_percentage": 37, "resets_at": time.time() + 3600}}, "private_body": "MUST_NOT_PERSIST"}).encode()
    env = {k: v for k, v in os.environ.items() if not (k.startswith("ANTHROPIC_") or k.startswith("CLAUDE_CODE_"))}
    result = subprocess.run([str(app), "--claude-statusline", identity, generation, str(profiles)],
                            input=payload, capture_output=True, env=env, timeout=10, check=False)
    assert result.returncode == 0, f"Helper exit={result.returncode}; stdout={result.stdout!r}; stderr={result.stderr!r}"
    assert result.stdout == payload, "Existing statusLine output changed"
    snapshots = list(folder.glob("snapshot-*.json"))
    assert len(snapshots) == 1
    saved = snapshots[0].read_text()
    assert "MUST_NOT_PERSIST" not in saved and "private_body" not in saved
    assert json.loads(saved)["fiveHour"]["usedPercent"] == 37
    before = snapshots[0].read_bytes()
    subprocess.run([str(app), "--claude-statusline", identity, str(uuid.uuid4()), str(profiles)],
                   input=payload, capture_output=True, env=env, timeout=10, check=True)
    assert snapshots[0].read_bytes() == before, "Old binding changed a snapshot"
    print("Packaged statusLine: output preserved, quota captured, secrets omitted, stale binding ignored — PASS")
