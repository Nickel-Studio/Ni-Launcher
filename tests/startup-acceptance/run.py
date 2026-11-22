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

from native_release import evidence_path

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
SCOPED_SOURCES = [
    "src-tauri/src/onboarding.rs", "src/lib/onboarding.ts",
    "src/components/startup/StartupGate.tsx", "src/components/startup/startup.css",
]
SHARED_INPUTS = [
    "src/App.tsx", "src/components/ui/Modal.tsx", "src-tauri/src/lib.rs",
    "src-tauri/build.rs", "CHANGELOG.json", "scripts/validate-release.mjs",
    "package.json", "src-tauri/Cargo.toml", "src-tauri/tauri.conf.json",
]


def hashes(paths):
    return {path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in paths}


def main():
    parser = argparse.ArgumentParser(description="Isolated production onboarding acceptance. No Git, native EXE execution, root dev server, or credentials.")
    parser.add_argument("--evidence-dir", type=Path)
    parser.add_argument("--channel", choices=["chromium", "msedge"], default="chromium")
    parser.add_argument("--executable-path", type=Path)
    parser.add_argument("--skip-rust", action="store_true")
    parser.add_argument("--only", help="Comma-separated browser case ids")
    parser.add_argument("--build-fixture", action="store_true")
    parser.add_argument("--build-production", action="store_true", help="Build the real frontend into this run's evidence, not root dist")
    args = parser.parse_args()
    target = args.evidence_dir or HERE / "evidence" / (datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ") + "-" + str(os.getpid()))
    evidence = evidence_path(target, new=True)
    evidence.mkdir(parents=True, exist_ok=False)
    environment = os.environ.copy()
    environment["NO_COLOR"] = "1"
    environment["PYTHONIOENCODING"] = "utf-8"
    environment["PYTHONDONTWRITEBYTECODE"] = "1"
    environment.pop("FORCE_COLOR", None)
    for key in list(environment):
        if key.startswith("VITE_"):
            environment.pop(key)
    stages = []
    before = hashes(SCOPED_SOURCES)
    shared_before = hashes(SHARED_INPUTS)
    harness_files = [path for path in HERE.iterdir() if path.is_file() and path.suffix in {".py", ".ts", ".tsx", ".mjs", ".html", ".json", ".css"}]
    harness_files.extend(path for path in (HERE / "fixture").rglob("*") if path.is_file())
    harness_paths = sorted(str(path.relative_to(ROOT)).replace("\\", "/") for path in harness_files)
    harness_before = hashes(harness_paths)
    node = shutil.which("node")
    if not node:
        raise RuntimeError("Node is required")

    def stage(name, command, timeout=240):
        print("Running " + name + ": " + subprocess.list2cmdline([str(part) for part in command]), flush=True)
        started = time.monotonic()
        try:
            process = subprocess.run(command, cwd=ROOT, env=environment, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout)
            code, stdout, stderr = process.returncode, process.stdout, process.stderr
        except subprocess.TimeoutExpired as error:
            code, stdout, stderr = 124, "", "Stage timeout: " + str(error)
        except Exception as error:
            code, stdout, stderr = 125, "", str(error)
        log = evidence / (name + "-stdout.txt")
        with log.open("x", encoding="utf-8") as stream:
            stream.write(stdout + "\nSTDERR\n" + stderr)
        stages.append({"stage": name, "command": [str(part) for part in command], "cwd": str(ROOT), "exitCode": code, "durationSeconds": round(time.monotonic() - started, 3), "stdoutEvidence": str(log)})
        print(stdout[-14000:], flush=True)
        if stderr:
            print(stderr[-8000:], flush=True)
        print(name + " exit=" + str(code), flush=True)
        return code

    stage("release-metadata", [node, "scripts/validate-release.mjs"])
    stage("production-typescript", [node, "node_modules/typescript/bin/tsc", "--noEmit"])
    stage("fixture-typescript", [node, "node_modules/typescript/bin/tsc", "--noEmit", "-p", "tests/startup-acceptance/tsconfig.json"])
    stage("native-script-help", [sys.executable, str(HERE / "native_release.py"), "--help"])
    stage("native-script-safety", [sys.executable, str(HERE / "test_native_safety.py")])
    if args.build_production:
        stage("production-build", [node, str(HERE / "build_frontend.mjs"), "--out-dir", str(evidence / "production-dist")])
    # Runtime and type checks are independent; any failed stage still fails the full report.
    fixture_ok = True
    if args.build_fixture:
        environment["STARTUP_FIXTURE_OUT"] = str(evidence / "fixture-dist")
        fixture_ok = stage("fixture-build", [node, "node_modules/vite/bin/vite.js", "build", "--config", "tests/startup-acceptance/vite.config.mjs"]) == 0
    if not args.skip_rust:
        cargo = shutil.which("cargo")
        environment["NICKEL_STARTUP_TEST_EVIDENCE"] = str(evidence / "rust-records")
        stage("rust-onboarding", [cargo or "cargo", "test", "--manifest-path", "src-tauri/Cargo.toml", "--lib", "onboarding::tests", "--", "--nocapture"], timeout=900)
    server, server_log = None, None
    try:
        if fixture_ok:
            with socket.socket() as listener:
                listener.bind(("127.0.0.1", 0))
                port = listener.getsockname()[1]
            base = "http://127.0.0.1:" + str(port)
            server_command = [node, str(ROOT / "node_modules/vite/bin/vite.js"), "--config", str(HERE / "vite.config.mjs"), "--host", "127.0.0.1", "--port", str(port), "--strictPort"]
            server_log = (evidence / "fixture-server-stdout.txt").open("x", encoding="utf-8")
            server = subprocess.Popen(server_command, cwd=ROOT, env=environment, stdout=server_log, stderr=subprocess.STDOUT)
            deadline = time.monotonic() + 45
            while time.monotonic() < deadline:
                if server.poll() is not None:
                    raise RuntimeError("Isolated Vite server exited before ready")
                try:
                    with urlopen(base, timeout=1) as response:
                        if response.status == 200:
                            break
                except Exception:
                    time.sleep(0.1)
            else:
                raise RuntimeError("Isolated Vite fixture startup timed out")
            stages.append({"stage": "fixture-server", "command": server_command, "cwd": str(ROOT), "fixtureRoot": str(HERE / "fixture"), "exitCode": 0, "ownedPid": server.pid, "watch": None, "optimizeEntries": [str(HERE / "fixture/index.html")]})
            browser_command = [sys.executable, str(HERE / "browser.py"), "--base-url", base, "--evidence-dir", str(evidence / "browser"), "--channel", args.channel]
            if args.only:
                browser_command.extend(["--only", args.only])
            if args.executable_path:
                browser_command.extend(["--executable-path", str(args.executable_path.resolve())])
            stage("browser", browser_command, timeout=600)
    except Exception as error:
        stages.append({"stage": "fixture-server", "exitCode": 125, "error": str(error)})
        print(str(error), flush=True)
    finally:
        if server is not None:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=10)
            stages.append({"stage": "fixture-server-cleanup", "exitCode": 0, "ownedPid": server.pid, "processExitCode": server.returncode})
        if server_log is not None:
            server_log.close()
    after = hashes(SCOPED_SOURCES)
    shared_after = hashes(SHARED_INPUTS)
    harness_after = hashes(harness_paths)
    changed = [path for path in before if before[path] != after[path]]
    shared_changed = [path for path in shared_before if shared_before[path] != shared_after[path]]
    harness_changed = [path for path in harness_before if harness_before[path] != harness_after[path]]
    report = {
        "scope": "production-onboarding-tests-and-headless-browser-simulated-ipc",
        "workspace": str(ROOT),
        "evidenceDirectory": str(evidence),
        "nativeAcceptance": False,
        "nativeExecuted": False,
        "version": "1.0.3-beta",
        "finishedAt": datetime.now(timezone.utc).isoformat(),
        "stages": stages,
        "sourcesBefore": before,
        "sourcesAfter": after,
        "harnessSources": harness_before,
        "harnessChangedDuringRun": harness_changed,
        "changedScopedSourcesDuringRun": changed,
        "sharedInputsBefore": shared_before,
        "sharedInputsAfter": shared_after,
        "sharedInputsChangedByConcurrentWork": shared_changed,
        "passed": all(item["exitCode"] == 0 for item in stages) and not changed and not shared_changed and not harness_changed,
        "omissions": ["No native release executable was started.", "Native script verification uses inert synthetic PE headers, never production EXEs.", "No production Tauri release build was requested by this runner.", "No real credentials or account files were read."],
    }
    with (evidence / "acceptance.json").open("x", encoding="utf-8") as stream:
        json.dump(report, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    print("Acceptance evidence: " + str(evidence / "acceptance.json"), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
