"""Build privately and exercise production launch UI without changing app outputs."""
import argparse
import functools
import hashlib
import http.server
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
SOURCE_FILES = [
    "src/pages/Home.tsx", "src/pages/Home.css", "src/components/launch/useLauncher.ts",
    "src/components/launch/LaunchPanel.tsx", "src/components/launch/launch.css",
    "src/components/launch/LaunchButton.tsx", "src/components/launch/ProgressBar.tsx",
    "src/components/ui/Modal.tsx", "src/lib/settings.ts", "src/lib/instances.ts",
    "src/styles/tokens.css", "src/styles/global.css", "src/styles/ui.css", "src/styles/shell.css",
]


def hashes():
    return {name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest() for name in SOURCE_FILES}


class QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, *args):
        pass


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--evidence-dir", required=True, type=Path)
    parser.add_argument("--channel", default="msedge")
    parser.add_argument("--smoke", action="store_true", help="Run only the essential launch paths")
    args = parser.parse_args()
    args.evidence_dir = args.evidence_dir.resolve()
    args.evidence_dir.mkdir(parents=True, exist_ok=False)
    before = hashes()
    node = shutil.which("node")
    if not node:
        raise RuntimeError("Node.js is required")
    stages = []

    def run(name, command, env=None):
        completed = subprocess.run(command, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False, timeout=180)
        (args.evidence_dir / (name + ".txt")).write_bytes(completed.stdout)
        stages.append({"name": name, "exitCode": completed.returncode, "log": name + ".txt"})
        print(name + ": " + str(completed.returncode), flush=True)
        return completed.returncode

    code = 1
    server = None
    scratch = Path(tempfile.mkdtemp(prefix="nickel-launch-acceptance-"))
    try:
        env = os.environ.copy()
        env["LAUNCH_FIXTURE_OUT"] = str(scratch / "dist")
        env["LAUNCH_FIXTURE_CACHE"] = str(scratch / "cache")
        for name, command in [
            ("production-types", [node, str(ROOT / "node_modules/typescript/bin/tsc"), "--noEmit"]),
            ("fixture-types", [node, str(ROOT / "node_modules/typescript/bin/tsc"), "--noEmit", "-p", str(HERE / "tsconfig.json")]),
            ("fixture-build", [node, str(ROOT / "node_modules/vite/bin/vite.js"), "build", "--config", str(HERE / "vite.config.mjs")]),
        ]:
            if run(name, command, env) != 0:
                raise SystemExit(1)
        handler = functools.partial(QuietHandler, directory=str(scratch / "dist"))
        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        browser_command = [sys.executable, "-B", str(HERE / "browser.py"), "--url", f"http://127.0.0.1:{server.server_port}/", "--channel", args.channel, "--evidence-dir", str(args.evidence_dir / "browser")]
        if args.smoke:
            browser_command.append("--smoke")
        code = run("browser", browser_command, env)
        print((args.evidence_dir / "browser.txt").read_text(encoding="utf-8", errors="replace"), flush=True)
    finally:
        if server is not None:
            server.shutdown()
            server.server_close()
        shutil.rmtree(scratch)
        after = hashes()
        result = {"scope": "current production source; isolated browser fixture; no native launch", "nativeAcceptance": False, "stages": stages, "sourceHashesBefore": before, "sourceHashesAfter": after, "sourceUnchangedDuringRun": before == after, "scratchRemoved": not scratch.exists(), "ownedServerClosed": True, "passed": code == 0 and before == after and all(stage["exitCode"] == 0 for stage in stages)}
        (args.evidence_dir / "acceptance.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    raise SystemExit(0 if result["passed"] else 1)


if __name__ == "__main__":
    main()
