#!/usr/bin/env python3
"""Local web app: drop a binary, watch the static analysis run, and view side-by-side dual runs.

This is a demo/visualization server for local use -- no auth, no persistence,
binds to 127.0.0.1 only.

  POST /api/upload          body = raw bytes, X-Filename header -> {"id": ...}
  POST /api/sample          load bundled sample_network.o -> {"id": ...}
  GET  /api/sample          same as POST /api/sample
  GET  /api/events/<id>     SSE stream of live analysis events + dual runs + heatmap
  GET  /api/report/<id>     final report JSON
  GET  /dist/<path>         compiled JS/CSS assets
  GET  /                    the UI

Usage: python3 webapp/server.py [--port 8765] [--delay 0.08]
"""
from __future__ import annotations

import argparse
import json
import mimetypes
import os
import re
import socketserver
import subprocess
import sys
import tempfile
import time
import urllib.parse
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
LIFTMAP = os.path.join(REPO, "liftmap")
sys.path.insert(0, LIFTMAP)
import ingest_tracer  # noqa: E402
import placement  # noqa: E402

SAMPLE_OBJ = os.path.join(LIFTMAP, "sample_network.o")
SAMPLE_C = os.path.join(LIFTMAP, "sample_network.c")
LIFT_RESULTS_PATH = os.path.join(LIFTMAP, "lift_results.json")

WEB = HERE
DIST = os.path.join(HERE, "dist")
JOBS: dict[str, dict] = {}
DELAY = 0.08  # pacing so the animation is watchable; analysis itself is fast


def ensure_sample_obj():
    if os.path.exists(SAMPLE_OBJ):
        return SAMPLE_OBJ
    if not os.path.exists(SAMPLE_C):
        return None
    cc = os.environ.get("CC", "cc")
    flags = ["-O1", "-fno-inline"]
    if sys.platform == "darwin":
        flags += ["-arch", "x86_64"]
    cmd = [cc] + flags + ["-c", SAMPLE_C, "-o", SAMPLE_OBJ]
    try:
        subprocess.run(cmd, check=True, capture_output=True)
        return SAMPLE_OBJ if os.path.exists(SAMPLE_OBJ) else None
    except Exception as e:
        sys.stderr.write(f"warn: failed to compile sample_network.o: {e}\n")
        return None


def get_precomputed_results():
    if os.path.isfile(LIFT_RESULTS_PATH):
        try:
            with open(LIFT_RESULTS_PATH) as fh:
                return json.load(fh)
        except Exception:
            pass
    return None


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # quieter logs
        sys.stderr.write("%s - %s\n" % (self.address_string(), fmt % args))

    def handle(self):
        try:
            super().handle()
        except (BrokenPipeError, ConnectionResetError):
            pass

    # --- helpers ---
    def _respond(self, code: int, body: bytes | str, ctype: str) -> None:
        if isinstance(body, str):
            body = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        self.wfile.write(body)

    def _json(self, code: int, obj) -> None:
        self._respond(code, json.dumps(obj), "application/json")

    def _file(self, full_path: str, ctype: str | None = None) -> None:
        if not os.path.isfile(full_path):
            return self._json(404, {"error": "not found"})
        if ctype is None:
            if full_path.endswith(".woff2"):
                ctype = "font/woff2"
            elif full_path.endswith(".woff"):
                ctype = "font/woff"
            elif full_path.endswith(".otf"):
                ctype = "font/otf"
            elif full_path.endswith(".js"):
                ctype = "application/javascript; charset=utf-8"
            elif full_path.endswith(".css"):
                ctype = "text/css; charset=utf-8"
            else:
                ctype, _ = mimetypes.guess_type(full_path)
                if not ctype:
                    ctype = "application/octet-stream"
                if ctype.startswith("text/") or ctype in ("application/javascript", "application/json"):
                    ctype += "; charset=utf-8"
        with open(full_path, "rb") as fh:
            self._respond(200, fh.read(), ctype)

    # --- routes ---
    def do_GET(self) -> None:
        path = urllib.parse.urlparse(self.path).path
        if path in ("/", "/index.html"):
            return self._file(os.path.join(WEB, "index.html"), "text/html; charset=utf-8")
        if path.startswith("/dist/"):
            rel = path[len("/dist/"):]
            return self._file(os.path.join(DIST, rel))
        if path.startswith("/assets/"):
            rel = path[len("/assets/"):]
            return self._file(os.path.join(DIST, "assets", rel))
        if path.startswith("/api/events/"):
            return self._stream(path[len("/api/events/"):])
        if path.startswith("/api/report/"):
            job = JOBS.get(path[len("/api/report/"):])
            if not job:
                return self._json(404, {"error": "unknown job"})
            return self._json(200, job.get("report") or {"status": "pending"})
        if path == "/api/sample":
            return self._handle_sample()
        if path == "/api/health":
            return self._json(200, {"ok": True, "jobs": len(JOBS), "sample_available": os.path.exists(SAMPLE_OBJ)})
        return self._json(404, {"error": "not found"})

    def do_POST(self) -> None:
        path = urllib.parse.urlparse(self.path).path
        if path == "/api/sample":
            return self._handle_sample()
        if path != "/api/upload":
            return self._json(404, {"error": "not found"})
        length = int(self.headers.get("Content-Length") or 0)
        if not length:
            return self._json(400, {"error": "empty upload"})
        data = self.rfile.read(length)
        name = self.headers.get("X-Filename") or "uploaded.bin"
        safe = "".join(ch for ch in os.path.basename(name)
                       if ch.isalnum() or ch in "._-") or "uploaded.bin"
        tmpdir = tempfile.mkdtemp(prefix="ingest-")
        path = os.path.join(tmpdir, safe)
        with open(path, "wb") as fh:
            fh.write(data)
        job_id = uuid.uuid4().hex[:12]
        JOBS[job_id] = {"id": job_id, "name": safe, "path": path, "report": None, "is_sample": False}
        self._json(200, {"id": job_id, "name": safe, "bytes": len(data)})

    def _handle_sample(self) -> None:
        sample_path = ensure_sample_obj()
        if not sample_path or not os.path.exists(sample_path):
            return self._json(500, {"error": "sample_network.o not available"})
        with open(sample_path, "rb") as fh:
            data = fh.read()
        job_id = uuid.uuid4().hex[:12]
        JOBS[job_id] = {
            "id": job_id,
            "name": "sample_network.o",
            "path": sample_path,
            "report": None,
            "is_sample": True,
        }
        self._json(200, {"id": job_id, "name": "sample_network.o", "bytes": len(data), "is_sample": True})

    def do_HEAD(self) -> None:
        self.do_GET()

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
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()

        def emit(stage: str, payload) -> None:
            # Tracer's own "done" is mid-stream; keep it distinct so the UI
            # does not treat it as job completion before function rows land.
            out_stage = "tracer_complete" if stage == "done" else stage
            msg = json.dumps({"stage": out_stage, "payload": payload})
            self.wfile.write(f"data: {msg}\n\n".encode())
            self.wfile.flush()
            if DELAY:
                time.sleep(DELAY)

        try:
            emit("start", {"name": job["name"], "id": job_id})
            # 1. Real ingest tracer analysis
            report = ingest_tracer.analyze_binary(job["path"], on_event=emit)

            # 2. Keyed / deterministic placement calculation
            lift_key = placement.load_key()
            lift_plan = placement.plan(report, lift_key)
            report["placement"] = lift_plan
            emit("placement", lift_plan)

            selected_set = set(lift_plan.get("selected", report.get("lift", [])))
            precomputed = get_precomputed_results()
            precomputed_by_name = {}
            if precomputed and "results" in precomputed:
                for r in precomputed["results"]:
                    precomputed_by_name[r["name"]] = r
                    precomputed_by_name[r["name"].lstrip("_")] = r

            # 3. Real per-function rows. Nothing here is invented: metrics come
            #    from the tracer; execution fields come from lift_results.json
            #    (produced by `make demo`) or are null when a lift has not run.
            mem_rows = []
            for f in report["functions"]:
                name = f["name"]
                is_lifted = name in selected_set or name.lstrip("_") in selected_set
                real = precomputed_by_name.get(name) or precomputed_by_name.get(name.lstrip("_"))
                hexbytes = (real or {}).get("hexbytes") or ""
                pulley_out = (real or {}).get("pulley") or ""
                observed = None
                for line in pulley_out.splitlines():
                    if "=" in line and "expected" in line:
                        # e.g. "_ip_id_hash(100, 50) = 51156  (expected 51156)"
                        rhs = line.split("=", 1)[1]
                        tok = rhs.split()[0] if rhs.split() else ""
                        try:
                            observed = int(tok, 0)
                        except ValueError:
                            observed = None
                row = {
                    "name": name,
                    "addr": f["addr"],
                    "size_bytes": f["size_bytes"],
                    "n_insn": f["n_insn"],
                    "signature": f["signature"],
                    "loop_density": f["loop_density"],
                    "call_fraction": f["call_fraction"],
                    "network_score": f["network_score"],
                    "lift_score": f.get("lift_score", 0.0),
                    "pure_compute": bool(f.get("pure_compute")),
                    "decision": "LIFT" if is_lifted else "KEEP_NATIVE",
                    "reason": f.get("reason", ""),
                    # source representation (x86_64 object)
                    "src_size": (real or {}).get("src_size") or (len(hexbytes) // 2 if hexbytes else f["size_bytes"]),
                    "src_insn": f["n_insn"],
                    # destination representation (Pulley wasm) — only if measured
                    "wasm_size": (real or {}).get("wasm_size"),
                    "runner_ms": (real or {}).get("runner_ms"),
                    "expected": (real or {}).get("expected"),
                    "observed": observed,
                    "expected_outs": (real or {}).get("expected_outs"),
                    "passed": bool((real or {}).get("passed")) if real else None,
                    "pulley_log": pulley_out or None,
                    "qemu_log": (real or {}).get("qemu") or None,
                    "lifted_measured": bool(real and real.get("passed")),
                }
                mem_rows.append(row)
                emit("function_row", row)

            emit("memory_map", {
                "binary": report.get("binary"),
                "functions": mem_rows,
                "span": max((r["addr"] + r["size_bytes"] for r in mem_rows), default=0),
            })

            measured = [r for r in mem_rows if r["lifted_measured"]]
            emit("perf", {
                "note": "Static metrics from Capstone. runner_ms is wall time of the Pulley runner process from make demo. No cycle counts.",
                "functions": [
                    {
                        "name": r["name"],
                        "decision": r["decision"],
                        "src_size": r["src_size"],
                        "src_insn": r["src_insn"],
                        "wasm_size": r["wasm_size"],
                        "runner_ms": r["runner_ms"],
                        "expected": r["expected"],
                        "observed": r["observed"],
                        "passed": r["passed"],
                    }
                    for r in mem_rows
                ],
                "measured_count": len(measured),
            })

            report["functions"] = mem_rows
            job["report"] = report

            emit("complete", {
                "total_functions": report["total_functions"],
                "lift": report["lift"],
                "keep_native": report["keep_native"],
                "placement": lift_plan,
                "measured_count": len(measured),
                "honest": True,
            })

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
