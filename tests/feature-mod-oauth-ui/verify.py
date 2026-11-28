"""Build browser fixtures and test them using installed Edge (no browser downloads)."""
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import os
import subprocess
from threading import Thread

ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = ROOT / "tests/feature-mod-oauth-ui/evidence"
EVIDENCE.mkdir(exist_ok=True)
os.environ["PYTHONUTF8"] = "1"


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, format, *args):
        pass


def run(command, log_name):
    with (EVIDENCE / log_name).open("w", encoding="utf-8") as log:
        result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=log)
    output = (EVIDENCE / log_name).read_text(encoding="utf-8", errors="replace")
    print(output[-6500:])
    if result.returncode:
        raise SystemExit(result.returncode)


run(["node", "node_modules/vite/bin/vite.js", "build", "--config",
     "tests/feature-mod-oauth-ui/vite.config.mjs"], "fixture-build.log")
handler = partial(QuietHandler, directory=str(ROOT / "node_modules/.feature-mod-oauth-ui-dist"))
server = ThreadingHTTPServer(("127.0.0.1", 0), handler)
thread = Thread(target=server.serve_forever, daemon=True)
thread.start()
url = f"http://127.0.0.1:{server.server_port}"
try:
    run(["python", "tests/feature-mod-oauth-ui/run.py", "--url", url], "browser.log")
    run(["python", "tests/community-dependencies/run-regression.py", "--url",
         url + "/tests/community-dependencies/regression.html"], "dependency-browser.log")
finally:
    server.shutdown()
    server.server_close()
    thread.join()
