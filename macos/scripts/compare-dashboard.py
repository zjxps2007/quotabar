"""Compare the original SwiftUI windows and menu popover with the Rust UI."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
from uuid import UUID

from importlib.util import module_from_spec, spec_from_file_location
import dashboard_actions
import compare_monitor

HERE = Path(__file__).resolve().parent
spec = spec_from_file_location("dashboard", HERE / "check-dashboard.py")
dashboard = module_from_spec(spec)
spec.loader.exec_module(dashboard)
REFERENCE = "b5b9de048062e50e9ab4973ca48b75d9bf0d0e53"
MAIN = """import AppKit
import Foundation
if CommandLine.arguments.contains("--reference-ui") {
let a = CommandLine.arguments
do {
    try MainActor.assumeIsolated {
        let app = NSApplication.shared
        app.setActivationPolicy(.accessory)
        app.finishLaunching()
        let url: (String) -> URL = { URL(fileURLWithPath: $0) }
        if a[1] == "--pixels" {
            try compareReference(original: url(a[2]), candidate: url(a[3]), output: url(a[4]))
        } else if a[2] == "popover" {
            try AppDelegate().capturePopoverReference(stateURL: url(a[1]), output: url(a[3]), nativeState: url(a[4]))
        } else {
            try AppDelegate().captureReference(stateURL: url(a[1]), route: a[2], output: url(a[3]))
            try compareReference(original: url(a[3]), candidate: url(a[4]), output: url(a[5]))
        }
    }
} catch {
    FileHandle.standardError.write(Data("Reference capture failed: \\(error)\\n".utf8))
    exit(1)
}
exit(0)
}
"""


def build_reference(work):
    archive = work / "reference.tar"
    subprocess.run(["git", "archive", "-o", archive, REFERENCE, "macos", "shared", "LICENSE", "NOTICE"],
                   cwd=dashboard.ROOT, check=True)
    subprocess.run(["tar", "-xf", archive, "-C", work], check=True)
    src = work / "macos/Sources/QuotaBar"
    with (src / "App.swift").open("a") as f:
        f.write("\n" + (HERE / "capture-visible-popover.swift").read_text())
        f.write("\n" + (HERE / "reference-dashboard.swift").read_text())
    main = src / "main.swift"
    main.write_text(MAIN + "\n" + main.read_text())
    # The reference opens one fresh sheet per capture. Initialize the original
    # segmented control at its user-selected provider without changing its view.
    views = src / "Views.swift"
    template = views.read_text()
    original = ".onAppear { if let target = store.loginTarget { alias = target.name } }"
    if template.count(original) != 1:
        raise ValueError("Unexpected reference connection view")
    views.write_text(template.replace(original, '.onAppear { if let target = store.loginTarget { alias = target.name }; if let selected = ProcessInfo.processInfo.environment["QUOTABAR_REFERENCE_PROVIDER"].flatMap(ProviderKind.init(rawValue:)) { provider = selected } }'))
    subprocess.run(["bash", work / "macos/scripts/build.sh"], cwd=work, check=True)
    return work / "artifacts/QuotaBar.app/Contents/MacOS/QuotaBar"


def compare_popovers(work, app, originals, results, save_results, library):
    probe = work / "check-popover"
    probe_source = work / "popover-probe.swift"
    probe_source.write_text((HERE / "capture-visible-popover.swift").read_text() + "\n" + (HERE / "check-popover.swift").read_text())
    subprocess.run(["swiftc", probe_source, "-I", dashboard.ROOT / "macos/Sources/CQuotaBar",
                    "-L", dashboard.ROOT / "target/release", "-lquotabar_native", "-o", probe], check=True)
    env = {**os.environ, **dashboard.fixture_environment(), "DYLD_LIBRARY_PATH": str(dashboard.ROOT / "target/release"),
           "DYLD_INSERT_LIBRARIES": str(library)}
    for side in ("original", "rust"):
        (dashboard.REPORT / "windows" / side).mkdir(parents=True, exist_ok=True)
    for theme in dashboard.THEMES:
        for kind in ("sample", "empty", "overflow"):
            name = f"popover-{kind}-{theme}"
            data = json.loads((dashboard.REPORT / f"fixture-{theme}.json").read_text())
            account = data["settings"]["accounts"][0]
            snapshot = data["snapshots"][1]
            if kind == "empty":
                data["settings"]["accounts"], data["snapshots"] = [], []
            elif kind == "overflow":
                data["settings"]["accounts"], data["snapshots"] = [], []
                for i in range(15):
                    identity = str(UUID(int=i + 1)).upper()
                    data["settings"]["accounts"].append({**account, "id": identity, "identity": f"sample-{i}",
                        "provider": ("codex", "claude", "gemini")[i % 3], "name": f"스크롤 확인 계정 {i+1} · 긴 이름 표시"})
                    data["snapshots"].extend((identity, snapshot))
            state, wire, result = (work / filename for filename in ("state.json", "wire.json", "pixels.json"))
            state.write_text(json.dumps(data, ensure_ascii=False))
            original, native = originals / f"{name}.png", dashboard.REPORT / f"{name}.png"
            try:
                def visible(side):
                    return {**env, "QUOTABAR_TEST_POPOVER_FRAME": str(dashboard.REPORT / "windows" / side / f"{name}.png")}
                subprocess.run([app, state, "popover", original, wire, "--demo", "--reference-ui"], env=visible("original"), timeout=30, check=True)
                native_env = visible("rust")
                if kind == "sample":
                    native_env["QUOTABAR_TEST_POPOVER_TEXT"] = str(dashboard.REPORT / "popover-text" / theme)
                subprocess.run([probe, wire, native], env=native_env, timeout=30, check=True)
                subprocess.run([app, "--pixels", original, native, result, "--reference-ui"], env=env, timeout=30, check=True)
                results[name] = json.loads(result.read_text())
                print(f"{name}: {results[name]['changedPixels']} different pixels", flush=True)
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                results[name] = {"identical": False, "error": str(error)}
                print(f"{name}: {error}", flush=True)
            save_results()


def main():
    report = dashboard.REPORT
    originals = report / "original"
    originals.mkdir(parents=True, exist_ok=True)
    if not all((report / f"fixture-{theme}.json").exists() for theme in dashboard.THEMES):
        dashboard.write_fixtures()
    results = {}
    def save_results():
        (report / "comparison.json").write_text(json.dumps(results, indent=2))
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        app = build_reference(work)
        library = dashboard_actions.build_driver(work)
        actions_match = dashboard_actions.compare(dashboard, work, app, json.loads((report / "fixture-light.json").read_text()), library)
        for theme in dashboard.THEMES:
            fixture = report / f"fixture-{theme}.json"
            for route in dashboard.ROUTES:
                name = f"{route}-{theme}"
                result = work / "comparison.json"
                try:
                    subprocess.run([app, fixture, route, originals / f"{name}.png", report / f"{name}.png", result,
                                    "--dashboard-helper", "--demo", "--reference-ui"], stdout=subprocess.DEVNULL, timeout=30, check=True,
                                   env={**os.environ, **dashboard.fixture_environment(),
                                        "QUOTABAR_REFERENCE_PROVIDER": route.removeprefix("connect-") if route.startswith("connect-") else "codex"})
                    results[name] = json.loads(result.read_text())
                    print(f"{name}: {results[name]['changedPixels']} different pixels", flush=True)
                except (OSError, ValueError, subprocess.SubprocessError) as error:
                    results[name] = {"identical": False, "error": str(error)}
                    print(f"{name}: {error}", flush=True)
                save_results()
        compare_popovers(work, app, originals, results, save_results, library)
        windows_match = dashboard_actions.compare_windows(dashboard, work, app, library)
        monitor_matches = compare_monitor.compare(dashboard, work, app)
    save_results()
    return 0 if actions_match and windows_match and monitor_matches and all(record["identical"] for record in results.values()) else 1


if __name__ == "__main__":
    raise SystemExit(main())
