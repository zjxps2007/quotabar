"""Use a disposable signing key and exercise the actual monitors' update transfers."""
import hashlib
import json
from pathlib import Path
import platform
import shutil
import stat
import subprocess
import time
import zipfile

VERSION = "2147483646.0.3"
TAG = "v" + VERSION
REPO = "https://github.com/zjxps2007/quotabar"
INVALID = "업데이트 파일의 서명을 확인하지 못했습니다. 현재 버전을 유지합니다."


def signed_fixture(work):
    directory = work / "signed-update-fixture"
    directory.mkdir()
    archive = directory / f"QuotaBar-{VERSION}-macOS-{platform.machine()}.zip"
    with zipfile.ZipFile(archive, "w") as zf:
        zf.writestr("fixture.txt", bytes(range(256)) * 1024)
    payload = archive.read_bytes()
    (directory / "corrupt.zip").write_bytes(payload[:-1] + bytes([payload[-1] ^ 1]))
    manifest = {"schemaVersion": 1, "version": VERSION, "build": 42,
                "assets": [{"name": archive.name, "sha256": hashlib.sha256(payload).hexdigest(), "bytes": len(payload)}]}
    (directory / "update-manifest.json").write_text(json.dumps(manifest))
    key = work / "disposable-update-key.pem"
    subprocess.run(["openssl", "ecparam", "-name", "prime256v1", "-genkey", "-noout", "-out", key], check=True)
    subprocess.run(["openssl", "pkey", "-in", key, "-pubout", "-out", directory / "public.pem"], check=True)
    sig = directory / "update-manifest.sig"
    subprocess.run(["openssl", "dgst", "-sha256", "-sign", key, "-out", sig, directory / "update-manifest.json"], check=True)
    (directory / "invalid.sig").write_bytes(bytes(64))
    return directory


def install_fixture(source, home, bundle):
    shutil.copytree(source, home / "update-test")
    keys = list((bundle / "Contents/Resources").rglob("UpdatePublicKey.pem"))
    assert len(keys) == 1 and keys[0].resolve().is_relative_to(bundle.resolve()), keys
    shutil.copyfile(source / "public.pem", keys[0])
    (home / "asset-requests.log").touch()


def exercise(wire, incoming, command, home, results):
    folder = home / "update-test"
    stages = home / "Library/Application Support/AIUsage Lite/Updates"
    trace = home / "asset-requests.log"
    archive = next(folder.glob("QuotaBar-*.zip"))
    digest = hashlib.sha256(archive.read_bytes()).hexdigest()
    base = f"{REPO}/releases/download/{TAG}/"
    feed = dict(tag_name=TAG, draft=False, prerelease=False, html_url=f"{REPO}/releases/tag/{TAG}", assets=[
        dict(name=archive.name, state="uploaded", size=archive.stat().st_size, digest="sha256:" + digest,
             browser_download_url=base + archive.name)])

    def respond(name, value):
        tmp = home / (name + ".tmp")
        tmp.write_text(json.dumps(value))
        tmp.replace(home / name)

    def wait_for(predicate):
        deadline = time.monotonic() + 12
        last = None
        while time.monotonic() < deadline:
            try:
                event = wire["receive"](incoming, deadline)
            except TimeoutError:
                break
            if "state" in event:
                state = event["state"]["_0"]
                last = state.get("appUpdate", {})
                if predicate(state):
                    return last
        raise AssertionError(f"Unexpected update state {last}, requests {trace.read_text().splitlines()}")

    # The dashboard remains visible while updates are staged. Both monitors
    # use the normal automatic-download path and defer installer handoff.
    command("visibility", True)
    command("automaticUpdates", True)
    wait_for(lambda s: s["settings"].get("automaticUpdates") is True)
    expected_log = []
    previous = None
    for step, signature, contents in [("reject invalid signature", "invalid.sig", archive.name),
                                      ("reject invalid archive", "update-manifest.sig", "corrupt.zip"),
                                      ("stage signed archive", "update-manifest.sig", archive.name)]:
        entries = [("manifest", "update-manifest.json", "update-manifest.json"),
                   ("signature", "update-manifest.sig", signature), ("archive", archive.name, contents)]
        respond("asset-responses.json", {base + path: dict(name=name, status=200, file=body) for name, path, body in entries})
        respond("release-response.json", dict(status=200, body=feed))
        command("checkAppUpdate")
        expected_log.extend(name + " 200" for name, _, _ in (entries[:2] if signature == "invalid.sig" else entries))
        phase = "ready" if step == "stage signed archive" else "failed"
        checked = wait_for(lambda s: (v := s.get("appUpdate", {})).get("phase") == phase and v.get("version") == VERSION
                           and v.get("checkedAt") is not None and v["checkedAt"] != previous)
        previous = checked["checkedAt"]
        assert checked.get("message") == (None if phase == "ready" else INVALID), checked
        assert trace.read_text().splitlines() == expected_log
        directories = [p for p in stages.iterdir() if p.is_dir()]
        assert len(directories) == int(phase == "ready"), directories
        row = dict(step=step, phase=phase, message=checked.get("message"), version=VERSION, staged=len(directories))
        if directories:
            stage = directories[0]
            for name in ("update-manifest.json", "update-manifest.sig", archive.name):
                assert (stage / name).read_bytes() == (folder / name).read_bytes()
                assert stat.S_IMODE((stage / name).stat().st_mode) == 0o600
            assert stat.S_IMODE(stage.stat().st_mode) == 0o700
            row.update(sha256=digest, bytes=archive.stat().st_size)
        results.append(row)
    # Checking again discards the old staged files before issuing the request.
    respond("release-response.json", dict(status=404, body={}))
    command("checkAppUpdate")
    wait_for(lambda s: s.get("appUpdate", {}).get("message") == "공개된 정식 릴리즈가 없습니다.")
    assert not any(p.is_dir() for p in stages.iterdir())
    results.append(dict(step="discard previous stage on next check", phase="idle", staged=0))
    command("automaticUpdates", False)
    wait_for(lambda s: s["settings"].get("automaticUpdates") is False)
