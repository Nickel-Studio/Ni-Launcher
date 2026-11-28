"""Build and serve isolated interaction fixtures, without a persistent dev process."""
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Thread
import os
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
os.environ['PYTHONUTF8'] = '1'


class QuietHandler(SimpleHTTPRequestHandler):
    def log_message(self, format, *args):
        pass


subprocess.run(['node', 'node_modules/vite/bin/vite.js', 'build', '--config', 'tests/ui-remaining/interactions.vite.config.mjs'], cwd=ROOT, check=True)
server = ThreadingHTTPServer(('127.0.0.1', 0), partial(QuietHandler, directory=str(ROOT / 'node_modules/.interactions-dist')))
thread = Thread(target=server.serve_forever, daemon=True)
thread.start()
try:
    subprocess.run([sys.executable, 'tests/ui-remaining/run-interactions.py', '--url', f'http://127.0.0.1:{server.server_port}'], cwd=ROOT, check=True)
finally:
    server.shutdown()
    server.server_close()
    thread.join()
