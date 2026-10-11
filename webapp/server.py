#!/usr/bin/env python3
"""Durable ISR run coordinator. One engine worker, independent of SSE subscribers.

Run with: .venv/bin/python webapp/server.py --port 8765
ISR_WORKER selects the built worker; ISR_STATE_DIR selects the SQLite journal.
"""

from __future__ import annotations

import argparse
import asyncio
from contextlib import asynccontextmanager
import hashlib
import json
import os
from pathlib import Path
import signal
import sqlite3
import time
import uuid

from starlette.applications import Starlette
from starlette.requests import Request
from starlette.responses import FileResponse, JSONResponse
from starlette.routing import Mount, Route
from starlette.staticfiles import StaticFiles
from sse_starlette import EventSourceResponse
import uvicorn
from pipeline import PipelineRuns, routes as pipeline_routes

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
TERMINAL = {"completed", "failed", "cancelled", "interrupted"}
TIERS = {
    "baby": {"bytes": 64, "connections": 0, "kernels": 1, "rounds": 1},
    "medium": {"bytes": 8 * 1024 * 1024, "connections": 4, "kernels": 4, "rounds": 128},
    "large": {
        "bytes": 512 * 1024 * 1024,
        "connections": 16,
        "kernels": 4,
        "rounds": 256,
    },
}
MAX_SECONDS = 1200


def encode(value):
    return json.dumps(value, separators=(",", ":"), allow_nan=False)


class Runs:
    def __init__(self, state: Path, worker: Path):
        state.mkdir(parents=True, exist_ok=True)
        self.worker = worker.resolve()
        self.db = sqlite3.connect(state / "runs.sqlite")
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=FULL")
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS runs(id TEXT PRIMARY KEY, snapshot TEXT NOT NULL, created REAL NOT NULL);
            CREATE TABLE IF NOT EXISTS events(run_id TEXT NOT NULL, seq INTEGER NOT NULL, body TEXT NOT NULL,
                PRIMARY KEY(run_id,seq));
        """)
        self.tasks: dict[str, asyncio.Task] = {}
        self.processes: dict[str, asyncio.subprocess.Process] = {}
        self.changed: dict[str, asyncio.Event] = {}
        self.cancel_requested: set[str] = set()
        for run_id, raw in self.db.execute("SELECT id,snapshot FROM runs").fetchall():
            if json.loads(raw)["status"] not in TERMINAL:
                self.append(
                    run_id,
                    {
                        "type": "terminal",
                        "status": "interrupted",
                        "message": "Coordinator restarted; live process state cannot be resumed.",
                    },
                )

    def snapshot(self, run_id):
        row = self.db.execute(
            "SELECT snapshot FROM runs WHERE id=?", (run_id,)
        ).fetchone()
        if not row:
            return None
        state = json.loads(row[0])
        if "progress_history" not in state:
            # Recover bounded chart history for older saved journals as well.
            recent = self.db.execute(
                "SELECT body FROM events WHERE run_id=? ORDER BY seq DESC LIMIT 256",
                (run_id,),
            ).fetchall()
            events = [json.loads(item[0]) for item in reversed(recent)]
            state["progress_history"] = [
                {"seconds": event["elapsed_ms"] / 1000, "bytes": event["bytes"]}
                for event in events
                if event["type"] == "progress"
            ][-120:]
        return state

    def append(self, run_id, event):
        snapshot = self.snapshot(run_id)
        if snapshot is None or snapshot["status"] in TERMINAL:
            return
        seq = snapshot["cursor"] + 1
        event = {**event, "v": 1, "run_id": run_id, "seq": seq, "time": time.time()}
        snapshot["cursor"] = seq
        kind = event["type"]
        if kind == "started":
            snapshot["engine"] = event
        elif kind == "stage":
            key = f"{event.get('kernel')}:{event['stage']}"
            snapshot["stages"][key] = event
        elif kind == "artifact":
            snapshot["artifacts"][str(event["kernel"])] = event
        elif kind in {"progress", "security", "context", "result"}:
            snapshot[kind] = event
            if kind == "progress":
                snapshot["progress_history"] = (
                    snapshot.get("progress_history", [])
                    + [{"seconds": event["elapsed_ms"] / 1000, "bytes": event["bytes"]}]
                )[-120:]
        elif kind == "terminal":
            snapshot["status"] = event["status"]
            snapshot["message"] = event.get("message", "")
            snapshot["finished"] = event["time"]
        elif kind == "cancelling":
            snapshot["status"] = "cancelling"
        if kind in {"error", "log"}:
            snapshot["logs"] = (snapshot["logs"] + [event])[-80:]
        with self.db:
            self.db.execute(
                "INSERT INTO events VALUES(?,?,?)", (run_id, seq, encode(event))
            )
            self.db.execute(
                "UPDATE runs SET snapshot=? WHERE id=?", (encode(snapshot), run_id)
            )
        if run_id in self.changed:
            self.changed[run_id].set()

    def create(self, tier, seed):
        if any(not task.done() for task in self.tasks.values()):
            return None
        # Keep at most 100 completed reports, all with a complete replay journal.
        old = self.db.execute(
            "SELECT id FROM runs ORDER BY created DESC LIMIT -1 OFFSET 99"
        ).fetchall()
        with self.db:
            for (run_id,) in old:
                self.db.execute("DELETE FROM events WHERE run_id=?", (run_id,))
                self.db.execute("DELETE FROM runs WHERE id=?", (run_id,))
                self.changed.pop(run_id, None)
                self.tasks.pop(run_id, None)
        run_id = uuid.uuid4().hex
        snapshot = {
            "id": run_id,
            "tier": tier,
            "seed": seed,
            "status": "running",
            "cursor": 0,
            "created": time.time(),
            "stages": {},
            "artifacts": {},
            "logs": [],
            "progress_history": [],
            "worker_sha256": hashlib.sha256(self.worker.read_bytes()).hexdigest(),
            "requested": TIERS[tier],
        }
        with self.db:
            self.db.execute(
                "INSERT INTO runs VALUES(?,?,?)",
                (run_id, encode(snapshot), snapshot["created"]),
            )
        self.changed[run_id] = asyncio.Event()
        self.append(run_id, {"type": "accepted", "tier": tier})
        self.tasks[run_id] = asyncio.create_task(self.run(run_id, tier, seed))
        return self.snapshot(run_id)

    async def stop(self, run_id):
        self.cancel_requested.add(run_id)
        process = self.processes.get(run_id)
        if process and process.returncode is None:
            if process.stdin:
                try:
                    process.stdin.write(b"cancel\n")
                    await process.stdin.drain()
                except (BrokenPipeError, ConnectionResetError):
                    pass
            try:
                await asyncio.wait_for(asyncio.shield(process.wait()), 2)
            except TimeoutError:
                self.kill(process)

    @staticmethod
    def kill(process):
        if process.returncode is None:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            except PermissionError:
                # A group may already have disappeared during child reaping on macOS.
                try:
                    process.kill()
                except ProcessLookupError:
                    pass

    async def run(self, run_id, tier, seed):
        process = None
        result = None
        worker_error = None
        worker_cancelled = False
        started = False
        secure = False
        oracle = None
        last_bytes = 0
        try:
            process = await asyncio.create_subprocess_exec(
                str(self.worker),
                "--tier",
                tier,
                "--seed",
                seed,
                "--managed",
                stdin=asyncio.subprocess.PIPE,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                start_new_session=True,
                limit=256 * 1024,
            )
            self.processes[run_id] = process
            if run_id in self.cancel_requested:
                await self.stop(run_id)

            async def stdout():
                nonlocal \
                    result, \
                    worker_error, \
                    worker_cancelled, \
                    started, \
                    secure, \
                    oracle, \
                    last_bytes
                count = 0
                async for line in process.stdout:
                    count += 1
                    if count > 12000:
                        raise ValueError("Worker event limit exceeded")
                    event = json.loads(line)
                    kind = event.get("type")
                    if event.get("v") != 1 or kind not in {
                        "started",
                        "stage",
                        "artifact",
                        "security",
                        "context",
                        "progress",
                        "result",
                        "cancelled",
                        "error",
                    }:
                        raise ValueError("Invalid worker protocol event")
                    if result is not None or worker_cancelled or worker_error:
                        raise ValueError("Worker emitted data after terminal result")
                    if kind == "started":
                        if (
                            started
                            or event.get("tier") != tier
                            or event.get("total_bytes") != TIERS[tier]["bytes"]
                            or event.get("backend") != "riscv64gc-unknown-none-elf"
                        ):
                            raise ValueError("Worker identity or work size mismatch")
                        if event.get("seed") != seed or any(
                            event.get(key) != value
                            for key, value in TIERS[tier].items()
                            if key != "bytes"
                        ):
                            raise ValueError(
                                "Worker seed or workload configuration mismatch"
                            )
                        oracle = event.get("oracle")
                        if oracle not in {
                            "native-x86-and-rust-reference",
                            "rust-reference (host is not x86_64)",
                        }:
                            raise ValueError("Worker oracle provenance missing")
                        started = True
                    if not started:
                        raise ValueError("Worker did not start its protocol")
                    if kind == "progress":
                        value = event.get("bytes")
                        if (
                            type(value) is not int
                            or not last_bytes <= value <= TIERS[tier]["bytes"]
                            or event.get("total_bytes") != TIERS[tier]["bytes"]
                        ):
                            raise ValueError("Invalid or regressing progress")
                        last_bytes = value
                    if kind == "result":
                        result = event
                        # Publish success only after the process exits cleanly.
                        continue
                    if kind == "cancelled":
                        worker_cancelled = True
                        continue
                    if kind == "error":
                        worker_error = event.get("message", "Engine failed")
                    if kind == "security":
                        secure = (
                            event.get("syscall_rejected") is True
                            and event.get("codegen_invoked") is False
                            and event.get("stage") == "audit"
                        )
                    self.append(run_id, event)

            async def stderr():
                count = 0
                async for line in process.stderr:
                    count += 1
                    if count <= 80:
                        self.append(
                            run_id,
                            {
                                "type": "log",
                                "message": line.decode(errors="replace")[
                                    :2000
                                ].rstrip(),
                            },
                        )
                    # Always drain, even after the retained diagnostic limit.

            async with asyncio.timeout(MAX_SECONDS):
                async with asyncio.TaskGroup() as group:
                    group.create_task(stdout())
                    group.create_task(stderr())
                    group.create_task(process.wait())
            if run_id in self.cancel_requested:
                self.append(
                    run_id,
                    {
                        "type": "terminal",
                        "status": "cancelled",
                        "message": "Run cancelled; partial work is not a pass.",
                    },
                )
            elif process.returncode != 0 or worker_error or worker_cancelled:
                raise ValueError(
                    worker_error or f"Worker exited unexpectedly ({process.returncode})"
                )
            elif (
                not result
                or not secure
                or result.get("bytes") != TIERS[tier]["bytes"]
                or result.get("verified") != TIERS[tier]["bytes"] // 64
                or result.get("mismatches") != 0
                or result.get("syscall_rejected") is not True
            ):
                raise ValueError("Worker exited without a complete verified result")
            elif (
                result.get("oracle") != oracle
                or result.get("trace_executions")
                != TIERS[tier]["bytes"] // 64 * TIERS[tier]["rounds"]
                or any(
                    type(result.get(key)) is not int or result[key] <= 0
                    for key in ("riscv_steps", "execution_ns", "compilation_ns")
                )
            ):
                raise ValueError("Result lacks matching execution counts or provenance")
            else:
                self.append(run_id, result)
                self.append(
                    run_id,
                    {
                        "type": "terminal",
                        "status": "completed",
                        "message": "Every record and context matched; syscall rejected before codegen.",
                    },
                )
        except asyncio.CancelledError:
            self.append(
                run_id,
                {
                    "type": "terminal",
                    "status": "interrupted",
                    "message": "Coordinator stopped during this run.",
                },
            )
            raise
        except Exception as error:
            self.append(run_id, {"type": "error", "message": str(error)})
            self.append(
                run_id, {"type": "terminal", "status": "failed", "message": str(error)}
            )
        finally:
            if process:
                self.kill(process)
                await process.wait()
            self.processes.pop(run_id, None)
            self.cancel_requested.discard(run_id)

    async def close(self):
        pending = [task for task in self.tasks.values() if not task.done()]
        for task in pending:
            task.cancel()
        await asyncio.gather(*pending, return_exceptions=True)
        self.db.close()


@asynccontextmanager
async def lifespan(app):
    app.state.runs = Runs(
        Path(os.environ.get("ISR_STATE_DIR", REPO / ".isr-state")),
        Path(os.environ.get("ISR_WORKER", REPO / "target/release/isr-demo")),
    )
    app.state.pipeline = PipelineRuns(
        Path(os.environ.get("ISR_STATE_DIR", REPO / ".isr-state")) / "pipeline-ui",
        Path(os.environ.get("ISR_PROCESS", REPO / "target/release/isr-process")),
        Path(os.environ.get("ISR_SERVER_FIXTURE", REPO / "target/fixtures/rust-network-server.elf")),
    )
    yield
    await app.state.pipeline.close()
    await app.state.runs.close()


def manager(request: Request) -> Runs:
    return request.app.state.runs


def error(message, status=400):
    return JSONResponse(
        {"error": message}, status_code=status, headers={"Cache-Control": "no-store"}
    )


def origin_allowed(request):
    origin = request.headers.get("origin")
    return origin is None or origin in {
        f"http://{request.headers.get('host')}",
        f"https://{request.headers.get('host')}",
    }


async def create(request):
    if not origin_allowed(request):
        return error("Cross-origin requests are not accepted", 403)
    if request.headers.get("content-type", "").split(";")[0] != "application/json":
        return error("Expected application/json", 415)
    body = bytearray()
    async for chunk in request.stream():
        body.extend(chunk)
        if len(body) > 512:
            return error("Request too large", 413)
    try:
        data = json.loads(body)
        if not isinstance(data, dict) or set(data) - {"tier", "seed"}:
            return error("Only tier and seed may be supplied")
        tier = data.get("tier")
        seed = data.get("seed", "42")
        if (
            tier not in TIERS
            or not isinstance(seed, str)
            or not seed.isascii()
            or not seed.isdigit()
            or not 0 <= int(seed) < 2**64
        ):
            return error(
                "Expected a known tier and an unsigned 64-bit decimal seed string"
            )
    except (ValueError, TypeError):
        return error("Invalid JSON request")
    runs = manager(request)
    if not runs.worker.is_file() or not os.access(runs.worker, os.X_OK):
        return error("Engine worker unavailable; build target/release/isr-demo", 503)
    snapshot = runs.create(tier, seed)
    return (
        JSONResponse(snapshot, status_code=201, headers={"Cache-Control": "no-store"})
        if snapshot
        else error("Another run is active", 409)
    )


async def snapshot(request):
    data = manager(request).snapshot(request.path_params["run_id"])
    return (
        JSONResponse(data, headers={"Cache-Control": "no-store"})
        if data
        else error("Run not found or expired", 404)
    )


async def cancel(request):
    if not origin_allowed(request):
        return error("Cross-origin requests are not accepted", 403)
    runs = manager(request)
    run_id = request.path_params["run_id"]
    state = runs.snapshot(run_id)
    if not state:
        return error("Run not found or expired", 404)
    if state["status"] not in TERMINAL:
        runs.append(run_id, {"type": "cancelling"})
        await runs.stop(run_id)
    return JSONResponse(runs.snapshot(run_id), headers={"Cache-Control": "no-store"})


async def events(request):
    runs = manager(request)
    run_id = request.path_params["run_id"]
    state = runs.snapshot(run_id)
    if not state:
        return error("Run not found or expired", 404)
    try:
        cursor = int(
            request.headers.get("last-event-id", request.query_params.get("after", "0"))
        )
        if cursor < 0 or cursor > state["cursor"]:
            return error("Invalid event cursor; reload the run snapshot", 409)
    except ValueError:
        return error("Invalid event cursor", 400)

    async def stream():
        nonlocal cursor
        changed = runs.changed.setdefault(run_id, asyncio.Event())
        while not await request.is_disconnected():
            changed.clear()
            # Bounded pages, never a per-subscriber unbounded queue.
            rows = runs.db.execute(
                "SELECT seq,body FROM events WHERE run_id=? AND seq>? ORDER BY seq LIMIT 128",
                (run_id, cursor),
            ).fetchall()
            for seq, body in rows:
                cursor = seq
                yield {"id": str(seq), "event": "update", "data": body, "retry": 1000}
            state = runs.snapshot(run_id)
            if state is None:
                yield {"event": "expired", "data": "{}"}
                return
            if cursor >= state["cursor"] and state["status"] in TERMINAL:
                return
            if not rows:
                try:
                    await asyncio.wait_for(changed.wait(), 1)
                except TimeoutError:
                    pass

    return EventSourceResponse(
        stream(),
        ping=5,
        send_timeout=15,
        headers={"Cache-Control": "no-cache, no-transform", "X-Accel-Buffering": "no"},
    )


async def health(request):
    runs = manager(request)
    ready = runs.worker.is_file() and os.access(runs.worker, os.X_OK)
    return JSONResponse(
        {
            "ok": ready,
            "engine": "in-process ISR",
            "backend": "riscv64gc-unknown-none-elf",
            "tiers": TIERS,
        },
        status_code=200 if ready else 503,
    )


async def index(request):
    return FileResponse(HERE / "index.html", headers={"Cache-Control": "no-store"})


app = Starlette(
    lifespan=lifespan,
    routes=[
        *pipeline_routes,
        Route("/api/health", health),
        Route("/api/runs", create, methods=["POST"]),
        Route("/api/runs/{run_id}", snapshot),
        Route("/api/runs/{run_id}/events", events),
        Route("/api/runs/{run_id}/cancel", cancel, methods=["POST"]),
        Route("/", index),
        Mount("/dist", StaticFiles(directory=HERE / "dist", check_dir=False)),
    ],
)

if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8765)
    args = parser.parse_args()
    uvicorn.run(
        app,
        host=args.host,
        port=args.port,
        access_log=False,
        timeout_graceful_shutdown=5,
    )
