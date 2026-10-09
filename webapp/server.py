#!/usr/bin/env python3
"""Local web app: drop a binary, watch the static analysis run.

This is a demo/visualization server for local use -- no auth, no persistence,
binds to 127.0.0.1 only.

  POST /api/upload          body = raw bytes, X-Filename header -> {"id": ...}
  GET  /api/events/<id>     SSE stream of live analysis events
  GET  /api/report/<id>     final report JSON
  GET  /                    the UI

The analysis itself is the real ingest tracer (liftmap/ingest_tracer.py); this
server only streams its events so the UI can animate what is actually happening.

Usage: python3 webapp/server.py [--port 8765] [--delay 0.12]
"""
from __future__ import annotations

import argparse
import json
import os
import socketserver
import sys
import tempfile
import time
import urllib.parse
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(REPO, "liftmap"))
import ingest_tracer  # noqa: E402

WEB = HERE
JOBS: dict[str, dict] = {}
DELAY = 0.12  # pacing so the animation is watchable; analysis itself is fast


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # quieter logs
        sys.stderr.write("%s - %s\n" % (self.address_string(), fmt % args))

    # --- helpers ---
    def _respond(self, code: int, body: bytes | str, ctype: str) -> None:
        if isinstance(body, str):
            body = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def _json(self, code: int, obj) -> None:
        self._respond(code, json.dumps(obj), "application/json")

    def _file(self, name: str, ctype: str) -> None:
        path = os.path.join(WEB, name)
        if not os.path.isfile(path):
            return self._json(404, {"error": "not found"})
        with open(path, "rb") as fh:
            self._respond(200, fh.read(), ctype)

    # --- routes ---
    def do_GET(self) -> None:
        path = urllib.parse.urlparse(self.path).path
        if path in ("/", "/index.html"):
            return self._file("index.html", "text/html; charset=utf-8")
        if path.startswith("/api/events/"):
            return self._stream(path[len("/api/events/"):])
        if path.startswith("/api/report/"):
            job = JOBS.get(path[len("/api/report/"):])
            if not job:
                return self._json(404, {"error": "unknown job"})
            return self._json(200, job.get("report") or {"status": "pending"})
        if path == "/api/health":
            return self._json(200, {"ok": True, "jobs": len(JOBS)})
        return self._json(404, {"error": "not found"})

    def do_POST(self) -> None:
        if urllib.parse.urlparse(self.path).path != "/api/upload":
            return self._json(404, {"error": "not found"})
        length = int(self.headers.get("Content-Length") or 0)
        if not length:
            return self._json(400, {"error": "empty upload"})
        data = self.rfile.read(length)
        name = self.headers.get("X-Filename") or "uploaded.bin"
        # Keep the original name for display, store under a safe temp path.
        safe = "".join(ch for ch in os.path.basename(name)
                       if ch.isalnum() or ch in "._-") or "uploaded.bin"
        tmpdir = tempfile.mkdtemp(prefix="ingest-")
        path = os.path.join(tmpdir, safe)
        with open(path, "wb") as fh:
            fh.write(data)
        job_id = uuid.uuid4().hex[:12]
        JOBS[job_id] = {"id": job_id, "name": safe, "path": path, "report": None}
        self._json(200, {"id": job_id, "name": safe, "bytes": len(data)})

    # --- SSE ---
    def _stream(self, job_id: str) -> None:
        job = JOBS.get(job_id)
        if not job:
            return self._json(404, {"error": "unknown job"})
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.send_header("Connection", "keep-alive")
        self.send_header("X-Accel-Buffering", "no")
        self.end_headers()

        def emit(stage: str, payload) -> None:
            msg = json.dumps({"stage": stage, "payload": payload})
            self.wfile.write(f"data: {msg}\n\n".encode())
            self.wfile.flush()
            if DELAY:
                time.sleep(DELAY)

        try:
            emit("start", {"name": job["name"]})
            report = ingest_tracer.analyze_binary(job["path"], on_event=emit)
            job["report"] = report
        except SystemExit as exc:
            emit("error", {"message": str(exc) or "analysis failed"})
        except Exception as exc:  # noqa: BLE001 - surface anything to the UI
            emit("error", {"message": f"{type(exc).__name__}: {exc}"})
        try:
            self.wfile.write(b"event: close\ndata: {}\n\n")
            self.wfile.flush()
        except (BrokenPipeError, ConnectionResetError):
            pass


class Server(ThreadingHTTPServer):
    daemon_threads = True
    allow_reuse_address = True


def main(argv: list[str]) -> int:
    global DELAY
    ap = argparse.ArgumentParser(description="local ingest-tracer web UI")
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--host", default="127.0.0.1")
    ap.add_argument("--delay", type=float, default=DELAY,
                    help="seconds between streamed events (animation pacing)")
    ns = ap.parse_args(argv)
    DELAY = ns.delay

    httpd = Server((ns.host, ns.port), Handler)
    print(f"ingest-tracer UI: http://{ns.host}:{ns.port}/  (ctrl-c to stop)")
    try:
        httpd.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        httpd.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
