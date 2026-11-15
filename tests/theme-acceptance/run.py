import argparse
import hashlib
import json
import os
import shutil
import socket
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from urllib.request import urlopen

ROOT = Path(__file__).resolve().parents[2]
TEST_ROOT = Path(__file__).resolve().parent
SOURCES = [
    "src/lib/theme.ts", "src/lib/settings.ts", "src/components/settings/accents.ts",
    "src/components/settings/AppearancePanel.tsx", "src/components/settings/CustomThemePanel.tsx",
    "src/components/settings/ThemePreview.tsx", "src/components/settings/settings.css",
    "src/components/settings/types.ts", "src/pages/Settings.tsx", "src/pages/More.tsx",
    "src/components/TitleBar.tsx", "src/components/ui/BackgroundLayer.tsx",
    "src/components/ui/ThemeArtwork.tsx", "src/components/ui/theme-artwork.css",
    "src/components/ui/Button.tsx", "src/styles/tokens.css", "src/styles/global.css",
    "src/styles/ui.css", "src/styles/shell.css", "src-tauri/src/portable_pack.rs",
    "tests/theme-acceptance/fixtures/app.html", "tests/theme-acceptance/fixtures/app.tsx",
    "tests/theme-acceptance/fixtures/fixture.css", "tests/theme-acceptance/fixtures/runtime.html",
    "tests/theme-acceptance/fixtures/runtime.ts", "tests/theme-acceptance/tests/theme.test.mjs",
    "tests/theme-acceptance/browser/acceptance.py", "tests/theme-acceptance/run.py",
    "tests/theme-acceptance/vite.config.mjs", "tests/theme-acceptance/tsconfig.json",
]


def free_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--base-url")
    parser.add_argument("--skip-rust", action="store_true")
    parser.add_argument("--evidence-dir", type=Path)
    args = parser.parse_args()
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    evidence = args.evidence_dir or TEST_ROOT / "evidence" / (stamp + "-" + str(os.getpid()))
    evidence.mkdir(parents=True, exist_ok=False)
    results = []
    environment = os.environ.copy()
    environment["NO_COLOR"] = "1"
    environment.pop("FORCE_COLOR", None)
    environment["PYTHONIOENCODING"] = "utf-8"
    environment.pop("RUSTC_WRAPPER", None)
    environment.pop("RUSTC_WORKSPACE_WRAPPER", None)
    node = shutil.which("node")
    pnpm = shutil.which("pnpm.cmd") or shutil.which("pnpm")
    cargo = shutil.which("cargo")
    if not node or not pnpm:
        raise RuntimeError("Node and pnpm must be available")
    sources_before = {source: hashlib.sha256((ROOT / source).read_bytes()).hexdigest() for source in SOURCES}

    def run_stage(name, command, timeout=180, cwd=ROOT):
        print("Running " + name + ": " + subprocess.list2cmdline([str(part) for part in command]), flush=True)
        started = time.monotonic()
        try:
            result = subprocess.run(command, cwd=cwd, env=environment, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout)
            code, stdout, stderr = result.returncode, result.stdout, result.stderr
        except subprocess.TimeoutExpired as error:
            code = 124
            stdout = error.stdout.decode("utf-8", errors="replace") if isinstance(error.stdout, bytes) else error.stdout or ""
            stderr = "Timeout after " + str(timeout) + " seconds"
        except Exception as error:
            code, stdout, stderr = 125, "", str(error)
        log = evidence / (name + ".txt")
        log.write_text(stdout + "\nSTDERR\n" + stderr, encoding="utf-8")
        entry = {"stage": name, "command": [str(part) for part in command], "cwd": str(cwd), "exitCode": code, "durationSeconds": round(time.monotonic() - started, 3), "log": str(log.resolve())}
        results.append(entry)
        print(stdout[-12000:], flush=True)
        if stderr:
            print(stderr[-6000:], flush=True)
        print(name + " exit=" + str(code), flush=True)
        return code

    run_stage("production-build", [pnpm, "run", "build"])
    run_stage("typescript", [pnpm, "exec", "tsc", "--noEmit"])
    run_stage("fixture-typescript", [pnpm, "exec", "tsc", "--noEmit", "-p", "tests/theme-acceptance/tsconfig.json"])
    run_stage("logic", [node, "--test", "--test-reporter=tap", "tests/theme-acceptance/tests/theme.test.mjs"])
    fixture_dist = evidence / "fixture-dist"
    environment["THEME_FIXTURE_OUT"] = str(fixture_dist)
    if not args.base_url:
        run_stage("fixture-build", [node, "node_modules/vite/bin/vite.js", "build", "--mode", "development", "--config", "tests/theme-acceptance/vite.config.mjs"])
    server, server_log = None, None
    try:
        base = args.base_url
        if not base:
            port = free_port()
            base = "http://127.0.0.1:" + str(port)
            server_log = (evidence / "server.txt").open("w", encoding="utf-8")
            server = subprocess.Popen([sys.executable, "-m", "http.server", str(port), "--bind", "127.0.0.1"], cwd=fixture_dist, env=environment, stdout=server_log, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                if server.poll() is not None:
                    raise RuntimeError("Fixture server exited before serving fixtures")
                try:
                    with urlopen(base + "/tests/theme-acceptance/fixtures/app.html", timeout=1) as response:
                        if response.status == 200:
                            break
                except Exception:
                    time.sleep(0.1)
            else:
                raise RuntimeError("Fixture server did not become ready")
        run_stage("browser", [sys.executable, str(TEST_ROOT / "browser/acceptance.py"), "--base-url", base, "--evidence-dir", str(evidence / "browser")], timeout=600)
    except Exception as error:
        results.append({"stage": "browser-server", "exitCode": 125, "error": str(error)})
    finally:
        if server is not None:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=10)
            results.append({"stage": "server-cleanup", "exitCode": 0, "ownedPid": server.pid, "processExitCode": server.returncode})
        if server_log is not None:
            server_log.close()
    if not args.skip_rust:
        if cargo:
            run_stage("rust-personalization", [cargo, "test", "--manifest-path", "src-tauri/Cargo.toml", "--lib", "personalization", "--", "--nocapture"], timeout=900)
        else:
            results.append({"stage": "rust-personalization", "exitCode": 127, "error": "cargo not found"})
    sources_after = {source: hashlib.sha256((ROOT / source).read_bytes()).hexdigest() for source in SOURCES}
    changed_during_run = [source for source in SOURCES if sources_before[source] != sources_after[source]]
    report = {
        "runId": evidence.name,
        "finishedAt": datetime.now(timezone.utc).isoformat(),
        "root": str(ROOT),
        "results": results,
        "sourcesBefore": sources_before,
        "sourcesAfter": sources_after,
        "changedDuringRun": changed_during_run,
        "rustCompilerWrapper": None,
        "passed": all(result["exitCode"] == 0 for result in results) and not changed_during_run,
    }
    (evidence / "acceptance.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print("Acceptance evidence: " + str((evidence / "acceptance.json").resolve()), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
