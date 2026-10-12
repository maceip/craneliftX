"""Process-runtime UI adapter. Ledger events retain their original IDs and payloads.

Coordinator lifecycle and test-client evidence use a separate SSE event type;
they never masquerade as engine stages. Subscribers replay the SQLite journal.
"""
from __future__ import annotations

import asyncio
import hashlib
import json
import os
from pathlib import Path
import signal
import sqlite3
import struct
import subprocess
import time
from urllib.parse import unquote, urlsplit
import uuid

from starlette.responses import FileResponse, JSONResponse
from starlette.routing import Route
from sse_starlette import EventSourceResponse

ROOT = Path(__file__).resolve().parents[1]
TERMINAL = {"completed", "failed", "cancelled", "interrupted"}
LIMIT = 64 * 1024 * 1024
# The browser demo and direct runtime acceptance exercise the same workload.
# Four full frames make the real work substantial without padding the binary
# or delaying execution. The empty final connection tests oversize rejection.
SERVER_REQUEST_SIZES = json.loads((ROOT / "webapp/network_workload.json").read_text())["request_sizes"]


def encode(value):
    return json.dumps(value, separators=(",", ":"))


def blank_group(group):
    return {**group, "count": 0, "active": 0, "started": 0, "completed": 0,
            "failed": 0, "duration_ns": 0, "last_event": None, "output": None}


def apply_event(state, event):
    """Bounded projection; exact, complete events are retained separately."""
    if event["seq"] != state["cursor"] + 1:
        raise ValueError("Engine event sequence has a gap or duplicate")
    if state.get("engine_run_id") not in (None, event["run_id"]):
        raise ValueError("Engine run identity changed")
    state["engine_run_id"] = event["run_id"]
    state["cursor"] = event["seq"]
    state["elapsed_ns"] = event["run_elapsed_ns"]
    group = state["groups"].get(event["group"])
    if group is None:
        raise ValueError("Unknown engine group")
    group["count"] += 1
    group["last_event"] = event
    kind = event["type"]
    status = event.get("status", event.get("state"))
    start = (kind == "stage" and status == "started") or kind == "syscall_started"
    end = (kind == "stage" and status in {"completed", "failed"}) or kind in {"syscall_completed", "syscall_failed"}
    if start:
        group["started"] += 1
        group["active"] += 1
    if end:
        group["active"] = max(0, group["active"] - 1)
        group["completed"] += int(status != "failed" and kind != "syscall_failed")
        group["duration_ns"] += event.get("elapsed_ns", 0)
    if status == "failed" or kind == "syscall_failed":
        group["failed"] += 1
    if not start:
        preferred = {"audit": "security", "decode": "harvest", "ingest": "input", "cache": "compilation_progress", "artifacts": "artifact"}
        if not group["output"] or preferred.get(event["group"]) in (None, kind) or group["output"]["type"] != preferred.get(event["group"]):
            group["output"] = event
    state["recent"] = (state["recent"] + [event])[-120:]
    if kind == "input":
        state["input"] = event
    if kind in {"progress", "compilation_progress", "execution", "result"}:
        counters = event.get("counters", event.get("report"))
        if counters is not None:
            state["counters"] = counters
    if kind == "compilation_progress":
        state["history"] = (state["history"] + [{"seq": event["seq"], "elapsed_ns": event["run_elapsed_ns"],
            "blocks": event["counters"]["compiled_blocks"], "bytes": event["counters"]["unique_lifted_bytes"]}])[-180:]
    if kind == "artifact":
        state["artifact_count"] += 1
    if kind == "result":
        state["engine_result"] = event


class PipelineRuns:
    def __init__(self, directory: Path, runner: Path, fixture: Path):
        self.root, self.runner, self.fixture = directory.resolve(), runner.resolve(), fixture.resolve()
        self.root.mkdir(parents=True, exist_ok=True)
        self.groups = []
        self.unavailable = None
        try:
            self.groups = json.loads(subprocess.check_output([str(self.runner), "--groups"], timeout=5))
            if len(self.groups) != 13 or len({g["id"] for g in self.groups}) != 13:
                raise ValueError("Runner group catalog is invalid")
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            self.unavailable = f"Build the process runner: {error}"
        self.db = sqlite3.connect(self.root / "pipeline.sqlite")
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute("PRAGMA synchronous=FULL")
        self.db.executescript("""
            CREATE TABLE IF NOT EXISTS runs(id TEXT PRIMARY KEY, body TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS events(run_id TEXT, seq INTEGER, body TEXT NOT NULL, PRIMARY KEY(run_id,seq));
        """)
        self.states, self.tasks, self.processes, self.changed = {}, {}, {}, {}
        for run_id, body in self.db.execute("SELECT id,body FROM runs").fetchall():
            state = json.loads(body)
            for (raw,) in self.db.execute("SELECT body FROM events WHERE run_id=? AND seq>? ORDER BY seq", (run_id, state["cursor"])):
                apply_event(state, json.loads(raw))
            self.states[run_id] = state
            if state["status"] not in TERMINAL:
                self.update(run_id, status="interrupted", message="Coordinator restarted. The engine result was not confirmed.")

    def snapshot(self, run_id):
        return self.states.get(run_id)

    def update(self, run_id, **fields):
        state = self.states[run_id]
        state.update(fields)
        state["revision"] += 1
        with self.db:
            self.db.execute("INSERT OR REPLACE INTO runs VALUES(?,?)", (run_id, encode(state)))
        self.changed.setdefault(run_id, asyncio.Event()).set()

    def append(self, run_id, event):
        state = self.states[run_id]
        if event["seq"] <= state["cursor"]:
            existing = self.db.execute("SELECT body FROM events WHERE run_id=? AND seq=?", (run_id, event["seq"])).fetchone()
            if existing is None or json.loads(existing[0]) != event:
                raise ValueError("Replayed event differs from durable journal")
            return
        apply_event(state, event)
        with self.db:
            self.db.execute("INSERT INTO events VALUES(?,?,?)", (run_id, event["seq"], encode(event)))
        self.changed.setdefault(run_id, asyncio.Event()).set()

    def create(self, mode, name, data=None, restart=None):
        if self.unavailable:
            raise RuntimeError(self.unavailable)
        if any(not task.done() for task in self.tasks.values()):
            raise RuntimeError("Another process run is active")
        if mode == "rust-server" and data is None and not self.fixture.is_file():
            raise RuntimeError("Build the Rust server with python3 scripts/build_web_fixture.py")
        if data is None:
            if mode != "rust-server":
                raise ValueError("Binary input is required")
            data = self.fixture.read_bytes()
        run_id = uuid.uuid4().hex
        work = self.root / run_id
        work.mkdir()
        binary = work / "input.elf"
        binary.write_bytes(data)
        state = {"id": run_id, "mode": mode, "input_name": name, "status": "starting", "revision": 0,
                 "created_unix_ns": str(time.time_ns()), "cursor": 0, "elapsed_ns": 0,
                 "groups": {g["id"]: blank_group(g) for g in self.groups}, "recent": [], "history": [],
                 "counters": {}, "artifact_count": 0, "engine_result": None, "engine_run_id": None,
                 "verification": None, "message": "Starting process runtime", "diagnostics": [],
                 "runner_sha256": hashlib.sha256(self.runner.read_bytes()).hexdigest()}
        if restart is not None:
            state["restart"] = restart
        self.states[run_id] = state
        self.update(run_id)
        self.tasks[run_id] = asyncio.create_task(self.run(run_id, binary))
        return state

    async def restart(self, run_id, focus_group):
        state = self.states[run_id]
        if focus_group not in state["groups"]:
            raise ValueError("Unknown operation group")
        if any(key != run_id and not task.done() for key, task in self.tasks.items()):
            raise RuntimeError("Another process run is active")
        # Reuse the retained input, including the exact fixture bytes of an old
        # demo. Validate it before stopping an active execution.
        data = (self.root / run_id / "input.elf").read_bytes()
        expected = state.get("input", {}).get("sha256")
        if expected and hashlib.sha256(data).hexdigest() != expected:
            raise ValueError("Retained binary differs from the recorded input")
        await self.cancel(run_id)
        task = self.tasks.get(run_id)
        if task is not None:
            await asyncio.wait_for(asyncio.shield(task), 10)
        return self.create(state["mode"], state["input_name"], data, restart={
            "source_run_id": run_id, "focus_group": focus_group, "kind": "fresh-execution"})

    @staticmethod
    def kill(process):
        if process and process.returncode is None:
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass

    async def cancel(self, run_id):
        state = self.states[run_id]
        if state["status"] not in TERMINAL:
            self.update(run_id, status="cancelling", message="Stopping the guest process")
            self.kill(self.processes.get(run_id))

    async def _consume(self, run_id, endpoint):
        while True:
            try:
                await self._subscribe(run_id, endpoint)
            except (OSError, asyncio.IncompleteReadError, asyncio.TimeoutError):
                pass
            process = self.processes.get(run_id)
            if not process or process.returncode is not None:
                return
            self.update(run_id, engine_connection="reconnecting")
            await asyncio.sleep(.2)

    async def _subscribe(self, run_id, endpoint):
        url = urlsplit(endpoint)
        if url.hostname != "127.0.0.1" or url.path != "/events":
            raise ValueError("Unexpected engine event endpoint")
        reader, writer = await asyncio.open_connection(url.hostname, url.port, limit=4 * 1024 * 1024 + 1024)
        try:
            cursor = self.states[run_id]["cursor"]
            writer.write(f"GET /events?after={cursor} HTTP/1.1\r\nHost: {url.netloc}\r\n\r\n".encode())
            await writer.drain()
            header = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 5)
            if not header.startswith(b"HTTP/1.1 200"):
                raise ValueError("Engine rejected the live subscription")
            self.update(run_id, engine_connection="live")
            event_id = None
            while line := await reader.readline():
                if line.startswith(b"id: "):
                    event_id = int(line[4:])
                elif line.startswith(b"data: "):
                    event = json.loads(line[6:])
                    if event["seq"] != event_id:
                        raise ValueError("Engine SSE identity mismatch")
                    self.append(run_id, event)
        finally:
            writer.close()
            await writer.wait_closed()

    async def run(self, run_id, binary):
        state = self.states[run_id]
        work = binary.parent
        process, stream_task, client_task, stderr_task = None, None, None, None
        try:
            process = await asyncio.create_subprocess_exec(str(self.runner), str(binary), "--ledger-dir", str(work / "ledger"),
                "--events-bind", "127.0.0.1:0", "--max-seconds", "120", "--recompile-every-syscalls", "25" if state["mode"] == "rust-server" else "0",
                stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE, start_new_session=True)
            self.processes[run_id] = process
            if state["status"] == "cancelling":
                self.kill(process)
            else:
                self.update(run_id, status="running", message="Receiving committed engine events")
            endpoint = asyncio.get_running_loop().create_future()

            async def stderr():
                while data := await process.stderr.readline():
                    line = data.decode(errors="replace").rstrip()
                    if line.startswith("ledger: ") and not state.get("ledger"):
                        path = Path(line[8:]).resolve()
                        if not path.is_relative_to(work):
                            raise ValueError("Engine journal outside its run directory")
                        self.update(run_id, ledger=str(path))
                    elif line.startswith("events: ") and not endpoint.done():
                        endpoint.set_result(line[8:])
                    else:
                        state["diagnostics"] = (state["diagnostics"] + [line[:2048]])[-20:]
                if not endpoint.done():
                    endpoint.set_exception(RuntimeError("Engine exited before opening its event stream"))

            stderr_task = asyncio.create_task(stderr())
            if state["mode"] == "rust-server":
                client_task = asyncio.create_task(self.network_client(run_id, process.stdout))
            else:
                async def drain():
                    total = 0
                    while chunk := await process.stdout.read(65536):
                        total += len(chunk)
                    self.update(run_id, stdout_bytes=total)
                client_task = asyncio.create_task(drain())
            stream_task = asyncio.create_task(self._consume(run_id, await asyncio.wait_for(endpoint, 5)))
            # Test-client failure must end the run rather than leave the guest waiting.
            async def finish():
                await asyncio.gather(process.wait(), client_task, stream_task)
            await asyncio.wait_for(finish(), 130)
            await stderr_task
            if state.get("ledger"):
                with Path(state["ledger"]).open() as journal:
                    for line in journal:
                        if line.endswith("\n"):
                            self.append(run_id, json.loads(line))
            result = state["engine_result"]
            if state["status"] == "cancelling":
                self.update(run_id, status="cancelled", message="Guest stopped. Unfinished stages remain interrupted.")
            elif process.returncode == 0 and result and result["status"] == "completed":
                self.update(run_id, status="completed", message="Engine completed and exited successfully")
            else:
                self.update(run_id, status="failed", message=(result or {}).get("error", f"Engine exited with code {process.returncode}; no successful result"))
        except asyncio.CancelledError:
            self.update(run_id, status="interrupted", message="Coordinator stopped; execution was interrupted")
            raise
        except Exception as error:
            self.update(run_id, status="cancelled" if state["status"] == "cancelling" else "failed", message=str(error))
        finally:
            self.kill(process)
            if process:
                await process.wait()
            for task in [stderr_task, stream_task, client_task]:
                if task and not task.done():
                    task.cancel()
            await asyncio.gather(*[t for t in [stderr_task, stream_task, client_task] if t], return_exceptions=True)
            self.processes.pop(run_id, None)
            self.update(run_id, finished_unix_ns=str(time.time_ns()))

    async def network_client(self, run_id, stdout):
        ready = await asyncio.wait_for(stdout.readexactly(6), 30)
        if ready[:4] != b"RV64":
            raise ValueError("Rust server did not emit its readiness record")
        port = struct.unpack("!H", ready[4:])[0]
        proof = {"source": "host TCP test client", "verified_responses": 0, "payload_bytes": 0,
                 "first_connection_responses": 0, "reconnects_on_first_connection": 0, "status": "running"}
        self.update(run_id, verification=dict(proof))
        for connection, sizes in enumerate(SERVER_REQUEST_SIZES):
            reader, writer = await asyncio.open_connection("127.0.0.1", port)
            try:
                if not sizes:
                    writer.write(struct.pack("<I", 65537))
                    await writer.drain()
                    if await reader.read(1) != b"":
                        raise ValueError("Oversize request was not rejected")
                for sequence, size in enumerate(sizes, 1):
                    payload = bytes((i * 73 + sequence * 29 + connection * 11) & 255 for i in range(size))
                    expected = bytearray()
                    checksum = 0xCBF29CE484222325 ^ sequence
                    for i, byte in enumerate(payload):
                        byte = (byte + i) & 255
                        byte = (((byte << 1) | (byte >> 7)) & 255) ^ 0xA5
                        expected.append(byte)
                        checksum = ((checksum ^ byte) * 0x100000001B3) & ((1 << 64) - 1)
                    frame = struct.pack("<I", size) + payload
                    writer.write(frame[:1])
                    await writer.drain()
                    await asyncio.sleep(0.03)
                    writer.write(frame[1:])
                    await writer.drain()
                    response = await asyncio.wait_for(reader.readexactly(size + 12), 30)
                    if response != struct.pack("<IQ", size, checksum) + expected:
                        raise ValueError("Rust server response differs from independent reference")
                    proof["verified_responses"] += 1
                    proof["payload_bytes"] += size
                    if connection == 0:
                        proof["first_connection_responses"] += 1
                    self.update(run_id, verification=dict(proof))
                    if sequence == 6:
                        await asyncio.sleep(0.2)
                if sizes:
                    writer.write_eof()
                    if await reader.read(1) != b"":
                        raise ValueError("Rust server did not honor EOF")
            finally:
                writer.close()
                await writer.wait_closed()
        proof["status"] = "passed"
        self.update(run_id, verification=dict(proof))
        await stdout.read()

    async def close(self):
        for task in self.tasks.values():
            if not task.done():
                task.cancel()
        await asyncio.gather(*self.tasks.values(), return_exceptions=True)
        self.db.close()


def manager(request):
    return request.app.state.pipeline


def response(data, status=200):
    return JSONResponse(data, status_code=status, headers={"Cache-Control": "no-store"})


def allowed(request):
    return request.headers.get("origin") in {None, f"http://{request.headers.get('host')}", f"https://{request.headers.get('host')}"}


async def config(request):
    runs = manager(request)
    return response({"ready": runs.unavailable is None, "error": runs.unavailable,
                     "groups": runs.groups, "server_available": runs.fixture.is_file(),
                     "max_upload_bytes": LIMIT, "profile": "Static Linux x86-64 ELF · scalar instructions · loopback TCP"})


async def create(request):
    if not allowed(request):
        return response({"error": "Cross-origin requests are not accepted"}, 403)
    runs = manager(request)
    if any(not t.done() for t in runs.tasks.values()):
        return response({"error": "Another process run is active"}, 409)
    media = request.headers.get("content-type", "").split(";")[0]
    if media not in {"application/json", "application/octet-stream"}:
        return response({"error": "Expected a binary upload or a Rust server request"}, 415)
    body = bytearray()
    async for chunk in request.stream():
        body.extend(chunk)
        if len(body) > (LIMIT if media == "application/octet-stream" else 512):
            return response({"error": "Input exceeds the upload limit"}, 413)
    try:
        if media == "application/json":
            if json.loads(body) != {"mode": "rust-server"}:
                raise ValueError("Expected mode: rust-server")
            state = runs.create("rust-server", "rust-network-server.elf")
        else:
            if not body:
                raise ValueError("Choose a nonempty binary")
            name = Path(unquote(request.headers.get("x-binary-name", "uploaded.elf"))).name[:120]
            state = runs.create("upload", name, bytes(body))
        return response(state, 201)
    except ValueError as e:
        return response({"error": str(e)}, 400)
    except (OSError, RuntimeError) as e:
        return response({"error": str(e)}, 409 if "active" in str(e) else 503)


async def history(request):
    states = sorted(manager(request).states.values(), key=lambda s: int(s["created_unix_ns"]), reverse=True)[:30]
    return response([{k: s[k] for k in ["id", "input_name", "mode", "status", "cursor", "created_unix_ns"]} for s in states])


async def snapshot(request):
    state = manager(request).snapshot(request.path_params["run_id"])
    return response(state) if state else response({"error": "Run not found"}, 404)


async def cancel(request):
    if not allowed(request):
        return response({"error": "Cross-origin requests are not accepted"}, 403)
    runs, run_id = manager(request), request.path_params["run_id"]
    if not runs.snapshot(run_id):
        return response({"error": "Run not found"}, 404)
    await runs.cancel(run_id)
    return response(runs.snapshot(run_id))


async def restart(request):
    if not allowed(request):
        return response({"error": "Cross-origin requests are not accepted"}, 403)
    runs, run_id = manager(request), request.path_params["run_id"]
    if not runs.snapshot(run_id):
        return response({"error": "Run not found"}, 404)
    body = bytearray()
    async for chunk in request.stream():
        body.extend(chunk)
        if len(body) > 512:
            return response({"error": "Restart request is too large"}, 413)
    try:
        value = json.loads(body)
        if not isinstance(value, dict) or set(value) != {"focus_group"} or not isinstance(value["focus_group"], str):
            raise ValueError("Expected focus_group")
        return response(await runs.restart(run_id, value["focus_group"]), 201)
    except ValueError as error:
        return response({"error": str(error)}, 400)
    except FileNotFoundError:
        return response({"error": "The original binary is no longer retained; open it again to run it"}, 410)
    except asyncio.TimeoutError:
        return response({"error": "The current run is still stopping; try again when it has ended"}, 409)
    except (OSError, RuntimeError) as error:
        return response({"error": str(error)}, 409)


async def events(request):
    runs, run_id = manager(request), request.path_params["run_id"]
    state = runs.snapshot(run_id)
    if not state:
        return response({"error": "Run not found"}, 404)
    try:
        cursor = int(request.headers.get("last-event-id", request.query_params.get("after", "0")))
        if cursor < 0 or cursor > state["cursor"]:
            raise ValueError()
    except ValueError:
        return response({"error": "Invalid event cursor; reload the snapshot"}, 409)

    async def stream():
        nonlocal cursor
        revision = -1
        changed = runs.changed.setdefault(run_id, asyncio.Event())
        while not await request.is_disconnected():
            changed.clear()
            rows = runs.db.execute("SELECT seq,body FROM events WHERE run_id=? AND seq>? ORDER BY seq LIMIT 64", (run_id, cursor)).fetchall()
            for seq, body in rows:
                cursor = seq
                yield {"id": str(seq), "event": "ledger", "data": body, "retry": 500}
            state = runs.snapshot(run_id)
            if cursor >= state["cursor"] and state["revision"] != revision:
                revision = state["revision"]
                meta = {k: state.get(k) for k in ["id", "status", "message", "revision", "verification", "finished_unix_ns", "diagnostics", "stdout_bytes", "engine_connection"]}
                yield {"event": "coordinator", "data": encode(meta)}
            if cursor >= state["cursor"] and state["status"] in TERMINAL:
                return
            if not rows:
                try:
                    await asyncio.wait_for(changed.wait(), 1)
                except TimeoutError:
                    pass
    return EventSourceResponse(stream(), ping=5, send_timeout=10,
        headers={"Cache-Control": "no-cache, no-transform", "X-Accel-Buffering": "no"})


async def journal(request):
    state = manager(request).snapshot(request.path_params["run_id"])
    if not state or not state.get("ledger"):
        return response({"error": "No engine journal yet"}, 404)
    return FileResponse(state["ledger"], media_type="application/x-ndjson", filename=f"chunder-{state['id']}.jsonl")


async def artifact(request):
    runs, run_id = manager(request), request.path_params["run_id"]
    state = runs.snapshot(run_id)
    name = request.path_params["name"]
    if not state or not state.get("ledger") or Path(name).name != name:
        return response({"error": "Artifact not found"}, 404)
    path = (Path(state["ledger"]).parent / name).resolve()
    if path.parent != Path(state["ledger"]).parent or not path.is_file() or path.suffix not in {".x86", ".clif", ".rv64"}:
        return response({"error": "Artifact not found"}, 404)
    return FileResponse(path, filename=name)


routes = [Route("/api/pipeline/config", config), Route("/api/pipeline/runs", history),
          Route("/api/pipeline/runs", create, methods=["POST"]),
          Route("/api/pipeline/runs/{run_id}", snapshot),
          Route("/api/pipeline/runs/{run_id}/events", events),
          Route("/api/pipeline/runs/{run_id}/cancel", cancel, methods=["POST"]),
          Route("/api/pipeline/runs/{run_id}/restart", restart, methods=["POST"]),
          Route("/api/pipeline/runs/{run_id}/journal", journal),
          Route("/api/pipeline/runs/{run_id}/artifacts/{name}", artifact)]
