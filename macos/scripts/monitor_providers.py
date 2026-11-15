"""Connect synthetic CLI accounts to the packaged monitor and inspect its actual quota state."""
import json
from pathlib import Path
import time
from uuid import uuid4

FIXTURES = Path(__file__).resolve().parents[2] / "shared/fixtures"
CODEX = "https://chatgpt.com/backend-api/wham/usage"
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

    def response(endpoint, status, body):
        responses[endpoint].update(status=status, body=body)
        tmp = directory / "provider-responses.tmp"
        tmp.write_text(json.dumps(responses))
        tmp.replace(directory / "provider-responses.json")

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
                if match := predicate(last):
                    return match
        summary = None if last is None else {k: last.get(k) for k in ("loading", "errors", "notes", "snapshots", "message")}
        raise TimeoutError(f"Unexpected provider state: {summary}; HTTP fixture trace: {trace.read_text().splitlines()}")

    def pairs(state, key):
        return dict(zip(state[key][::2], state[key][1::2]))

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

        def snapshot(state, expected, error, note):
            account = next((a for a in state["settings"]["accounts"] if a["name"] == alias), None)
            if not account:
                return None
            account_id = account["id"]
            value = pairs(state, "snapshots").get(account_id, {})
            if (account_id in state["loading"] or [w["remaining"] for w in value.get("windows", [])] != expected
                    or pairs(state, "errors").get(account_id) != error or pairs(state, "notes").get(account_id) != note):
                return None
            return account_id, value

        def check(step, expected, error=None, note=None):
            account_id, value = until(lambda state: snapshot(state, expected, error, note))
            results.append({"step": step, "provider": provider, "plan": value.get("plan"), "email": value.get("email"),
                            "windows": value["windows"], "error": error, "note": note})
            return account_id, value

        account_id, previous = check("connected", remaining)
        response(url, 401 if provider == "codex" else 403,
                 {} if provider == "codex" else {"error": {"reason": "UNSUPPORTED_CLIENT"}})
        command("refresh", account_id)
        _, cached = check("access denied, last successful quota retained", remaining, failure)
        assert cached == previous, provider
        revised = remaining.copy()
        if provider == "codex":
            quota[provider]["rate_limit"]["primary_window"]["used_percent"] = 7
            revised[0] = 93
        else:
            quota[provider]["buckets"][1]["remainingFraction"] = 0.12
            revised[0] = 12
        response(url, 200, quota[provider])
        command("refresh", account_id)
        check("retry recovered", revised)
        if provider == "codex":
            switched = {**login, "tokens": {"access_token": "another-fixture", "account_id": "other-workspace"}}
            source.write_text(json.dumps(switched))
            command("refresh", account_id)
            check("CLI account changed, pinned account retained", revised, note=OTHER_ACCOUNT)
        command("remove", account_id)
        command("message", "removed " + alias)
        until(lambda state: state.get("message") == "removed " + alias
              and not any(a["id"] == account_id for a in state["settings"]["accounts"])
              and account_id not in pairs(state, "snapshots"))
        vault = directory / "Library/Application Support/AIUsage Lite/Credentials" / (account_id + ".json")
        assert not vault.exists() and source.is_file(), provider
        results.append({"step": "removed, source preserved", "provider": provider})
    requests = trace.read_text().splitlines()
    assert requests == ["codex 200", "codex 401", "codex 200", "codex 200",
                        "assist 200", "gemini 200", "assist 200", "gemini 403", "assist 200", "gemini 200"], requests
    return requests
