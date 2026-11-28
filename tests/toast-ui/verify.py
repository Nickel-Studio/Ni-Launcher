"""Build an isolated fixture and run Edge against a temporary local HTTP server."""
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Thread
import os
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
EVIDENCE = ROOT / 'tests/toast-ui/evidence'
EVIDENCE.mkdir(exist_ok=True)
os.environ['PYTHONUTF8'] = '1'


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, format, *args):
        pass


def run(command, log_name):
    with (EVIDENCE / log_name).open('w', encoding='utf-8') as log:
        result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=log)
    print((EVIDENCE / log_name).read_text(encoding='utf-8', errors='replace')[-6500:])
    if result.returncode:
        raise SystemExit(result.returncode)


run(['node', 'node_modules/vite/bin/vite.js', 'build', '--config', 'tests/toast-ui/vite.config.mjs'], 'build.log')
server = ThreadingHTTPServer(('127.0.0.1', 0), partial(QuietHandler, directory=str(ROOT / 'node_modules/.toast-ui-dist')))
thread = Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    run([sys.executable, 'tests/toast-ui/run.py', '--url', f'http://127.0.0.1:{server.server_port}'], 'browser.log')
finally:
    server.shutdown()
    server.server_close()
    thread.join()
