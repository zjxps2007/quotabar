"""Connect synthetic CLI accounts to the packaged monitor and inspect its actual quota state."""
import base64
import json
from pathlib import Path
import time
from uuid import uuid4

FIXTURES = Path(__file__).resolve().parents[2] / "shared/fixtures"
CODEX = "https://chatgpt.com/backend-api/wham/usage"
REFRESH = "https://auth.openai.com/oauth/token"
ASSIST = "https://cloudcode-pa.googleapis.com/v1internal:loadCodeAssist"
GEMINI = "https://cloudcode-pa.googleapis.com/v1internal:retrieveUserQuota"
DENIED = "로그인이 만료되었거나 접근이 거부되었습니다. Codex는 앱에서 다시 로그인하거나, 해당 CLI의 로그인 파일을 갱신하세요."
UNSUPPORTED = "이 계정의 Gemini CLI 접근이 지원되지 않습니다. Code Assist Standard 또는 Enterprise가 필요합니다."
OTHER_ACCOUNT = "CLI가 다른 계정으로 전환되어 보관된 로그인을 사용합니다."


def exercise(wire, incoming, outgoing, directory, results):
    directory = Path(directory)
    trace = directory / "provider-requests.log"
    trace.touch()
    codex_token, gemini_token = "fixture-codex-access", "fixture-gemini-access"
    codex = {"tokens": {"access_token": codex_token, "account_id": "fixture-workspace"},
             "user_id": "fixture-user", "email": "codex@example.invalid"}
    gemini = {"access_token": gemini_token, "email": "gemini@example.invalid"}
    responses = {
        CODEX: dict(name="codex", token=codex_token, account="fixture-workspace", method="GET"),
        ASSIST: dict(name="assist", token=gemini_token, method="POST", status=200,
                     body={"currentTier": {"id": "standard"}, "cloudaicompanionProject": "fixture-project"}),
        GEMINI: dict(name="gemini", token=gemini_token, method="POST"),
    }
    quota = {provider: json.loads((FIXTURES / (provider + "-quota.json")).read_bytes())
             for provider in ("codex", "gemini")}

    def publish():
        tmp = directory / "provider-responses.tmp"
        tmp.write_text(json.dumps(responses))
        tmp.replace(directory / "provider-responses.json")

    def response(endpoint, status, body):
        responses[endpoint].update(status=status, body=body)
        publish()

    def command(name, value):
        wire["send"](outgoing, {name: {"_0": value}})

    def until(predicate):
        deadline = time.monotonic() + 12
        last = None
        while time.monotonic() < deadline:
            try:
                event = wire["receive"](incoming, deadline)
            except TimeoutError:
                break
            if "state" in event:
                last = event["state"]["_0"]
                assert "fixture-replacement-" not in json.dumps(last), "CLI login leaked into dashboard state"
                if match := predicate(last):
                    return match
        summary = None if last is None else {k: last.get(k) for k in ("loading", "errors", "notes", "snapshots", "message")}
        raise TimeoutError(f"Unexpected provider state: {summary}; HTTP fixture trace: {trace.read_text().splitlines()}")

    def pairs(state, key):
        return dict(zip(state[key][::2], state[key][1::2]))

    def snapshot(state, alias, expected, error, note):
        account = next((a for a in state["settings"]["accounts"] if a["name"] == alias), None)
        if not account:
            return None
        account_id = account["id"]
        value = pairs(state, "snapshots").get(account_id, {})
        if (account_id in state["loading"] or [w["remaining"] for w in value.get("windows", [])] != expected
                or pairs(state, "errors").get(account_id) != error or pairs(state, "notes").get(account_id) != note):
            return None
        return account_id, value

    def check(provider, alias, step, expected, error=None, note=None):
        account_id, value = until(lambda state: snapshot(state, alias, expected, error, note))
        results.append({"step": step, "provider": provider, "plan": value.get("plan"), "email": value.get("email"),
                        "windows": value["windows"], "error": error, "note": note})
        return account_id, value

    for provider, login, url, remaining, failure in (
        ("codex", codex, CODEX, [79, 47, 100], DENIED),
        ("gemini", gemini, GEMINI, [40, 90, 100], UNSUPPORTED),
    ):
        alias = provider + " import probe"
        source = directory / (provider + "-fixture-login.json")
        source.write_text(json.dumps(login))
        response(url, 200, quota[provider])
        request = str(uuid4()).upper()
        wire["send"](outgoing, {"connect": {"id": request, "provider": provider, "alias": alias, "path": str(source)}})
        reply = wire["wait_for"](incoming, "reply")
        assert reply["id"] == request and not reply.get("error"), reply
        command("message", alias)

        account_id, previous = check(provider, alias, "connected", remaining)
        vault = directory / "Library/Application Support/AIUsage Lite/Credentials" / (account_id + ".json")
        saved = vault.read_bytes()
        token = "fixture-replacement-" + provider
        if provider == "codex":
            replacement = {**login, "tokens": {**login["tokens"], "access_token": token}}
        else:
            replacement = {**login, "access_token": token}
            responses[ASSIST]["token"] = token
        source.write_text(json.dumps(replacement))
        responses[url]["token"] = token
        response(url, 401 if provider == "codex" else 403,
                 {} if provider == "codex" else {"error": {"reason": "UNSUPPORTED_CLIENT"}})
        command("refresh", account_id)
        _, cached = check(provider, alias, "access denied, last successful quota retained", remaining, failure)
        assert cached == previous, provider
        assert vault.read_bytes() == saved, "A failed fetch replaced the stored CLI login"
        revised = remaining.copy()
        if provider == "codex":
            quota[provider]["rate_limit"]["primary_window"]["used_percent"] = 7
            revised[0] = 93
        else:
            quota[provider]["buckets"][1]["remainingFraction"] = 0.12
            revised[0] = 12
        response(url, 200, quota[provider])
        command("refresh", account_id)
        check(provider, alias, "retry recovered", revised)
        assert json.loads(vault.read_bytes()) == replacement, "Successful quota did not commit the new CLI login"
        results.append({"step": "updated CLI login committed only after successful quota", "provider": provider})
        if provider == "codex":
            switched = {**login, "tokens": {"access_token": "another-fixture", "account_id": "other-workspace"}}
            source.write_text(json.dumps(switched))
            command("refresh", account_id)
            check(provider, alias, "CLI account changed, pinned account retained", revised, note=OTHER_ACCOUNT)
        command("remove", account_id)
        command("message", "removed " + alias)
        until(lambda state: state.get("message") == "removed " + alias
              and not any(a["id"] == account_id for a in state["settings"]["accounts"])
              and account_id not in pairs(state, "snapshots"))
        assert not vault.exists() and source.is_file(), provider
        results.append({"step": "removed, source preserved", "provider": provider})
    alias, provider = "Managed Codex fixture", "codex-managed"
    first, second, third = ("fixture-managed-" + label for label in ("first", "second", "third"))
    account_key = "fixture-managed-workspace"
    fresh_quota = json.loads((FIXTURES / "codex-quota.json").read_bytes())

    def configure(access_responses, refresh_token, refresh_status, refresh_body):
        responses[CODEX] = {"byBearer": {
            "Bearer " + token: dict(name="managed-quota", token=token, stored=token, account=account_key,
                                    method="GET", status=status, body=body)
            for token, status, body in access_responses}}
        responses[REFRESH] = dict(name="managed-rotation", method="POST", refresh=refresh_token,
                                  status=refresh_status, body=refresh_body)
        publish()

    def login(access, refresh, expiry, replacing=None):
        auth = {"tokens": {"account_id": account_key, "access_token": access, "refresh_token": refresh},
                "user_id": "fixture-managed-user", "expires_at": int(time.time()) + expiry}
        request_id = str(uuid4()).upper()
        payload = dict(id=request_id, alias=alias, data=base64.b64encode(json.dumps(auth).encode()).decode())
        if replacing is not None:
            payload["replacing"] = replacing
        wire["send"](outgoing, {"codexLogin": payload})
        reply = wire["wait_for"](incoming, "reply")
        assert reply["id"] == request_id and not reply.get("error"), reply
        command("message", "managed login submitted")

    configure([(first, 401, {}), (second, 200, fresh_quota)], "fixture-r1", 200,
              dict(access_token=second, refresh_token="fixture-r2", expires_in=7200))
    login(first, "fixture-r1", 7200)
    account_id, previous = check(provider, alias, "401, rotated login saved before quota retry", [79, 47, 100])
    vault = directory / "Library/Application Support/AIUsage Lite/Credentials" / (account_id + ".json")
    assert json.loads(vault.read_bytes())["tokens"]["refresh_token"] == "fixture-r2"
    configure([(second, 403, {})], "fixture-r2", 400, {})
    command("refresh", account_id)
    denied = "로그인이 만료되거나 해제되었습니다. 앱에서 다시 로그인하세요."
    _, cached = check(provider, alias, "revoked refresh retained last successful quota", [79, 47, 100], denied)
    assert cached == previous
    fresh_quota["rate_limit"]["primary_window"]["used_percent"] = 6
    configure([(third, 200, fresh_quota)], "fixture-r3", 200,
              dict(access_token=third, refresh_token="fixture-r4", expires_in=7200))
    login("fixture-expired-reconnection", "fixture-r3", -60, replacing=account_id)
    reconnected, _ = check(provider, alias, "same account reconnected, expired login rotated before first quota", [94, 47, 100])
    assert reconnected == account_id and json.loads(vault.read_bytes())["tokens"]["refresh_token"] == "fixture-r4"
    command("remove", account_id)
    command("message", "removed managed fixture")
    until(lambda state: state.get("message") == "removed managed fixture"
          and not any(a["id"] == account_id for a in state["settings"]["accounts"])
          and account_id not in pairs(state, "snapshots"))
    assert not vault.exists()
    results.append({"step": "removed managed login and vault copy", "provider": provider})
    requests = trace.read_text().splitlines()
    assert requests == ["codex 200", "codex 401", "codex 200", "codex 200",
                        "assist 200", "gemini 200", "assist 200", "gemini 403", "assist 200", "gemini 200",
                        "managed-quota 401", "managed-rotation 200", "managed-quota 200",
                        "managed-quota 403", "managed-rotation 400", "managed-rotation 200", "managed-quota 200"], requests
    return requests
