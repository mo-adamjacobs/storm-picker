"""Serve the Storm picker page locally. The analysis itself runs in the browser.

Any static file server works if it serves lightning/static/ at / (including wheels/, the
Rust sweep built for Pyodide) and the Python modules in lightning/ at /py/ (the browser
worker loads them into Pyodide). This one does that.

Run: uv run storm-picker   (http://127.0.0.1:8765)
"""
from __future__ import annotations

import argparse
import webbrowser
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

PKG = Path(__file__).parent
STATIC = PKG / "static"
TYPES = {".html": "text/html; charset=utf-8", ".js": "text/javascript", ".css": "text/css",
         ".py": "text/plain; charset=utf-8", ".json": "application/json", ".whl": "application/zip"}
WHEELS = STATIC / "wheels"


class Handler(BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        pass

    def do_GET(self):
        path = self.path.split("?", 1)[0].split("#", 1)[0]
        if path in ("/", "/index.html"):
            f = STATIC / "index.html"
        elif path.startswith("/py/"):
            f = PKG / path[len("/py/"):]
        else:
            f = STATIC / path.lstrip("/")
        # Only plain files directly inside these folders, with known types.
        if (f.parent not in (STATIC, PKG, WHEELS) or f.suffix not in TYPES or not f.is_file()
                or (f.parent == PKG and f.suffix != ".py")):
            self.send_error(404)
            return
        data = f.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", TYPES[f.suffix])
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--no-browser", action="store_true")
    a = ap.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", a.port), Handler)
    url = f"http://127.0.0.1:{a.port}/"
    print(f"Storm picker on {url}", flush=True)
    if not a.no_browser:
        webbrowser.open(url)
    server.serve_forever()


if __name__ == "__main__":
    main()
