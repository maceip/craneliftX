"""Real process runner checks for the web adapter; no simulated stage success."""
import asyncio
import json
from pathlib import Path
import struct
import sys
import tempfile
import unittest
from urllib.parse import urlsplit

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "webapp"))
from pipeline import PipelineRuns


def elf(code):
    image = bytearray(256 + len(code))
    image[:7] = b"\x7fELF\x02\x01\x01"
    struct.pack_into("<HHIQQQ", image, 16, 2, 62, 1, 0x20100, 64, 0)
    struct.pack_into("<HHHHHH", image, 52, 64, 56, 1, 64, 0, 0)
    struct.pack_into("<IIQQQQQQ", image, 64, 1, 5, 0, 0x20000, 0x20000, len(image), len(image), 4096)
    image[256:] = code
    return bytes(image)


class Pipeline(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.path = Path(self.temp.name)
        self.runner = ROOT / "target/release/isr-process"
        self.fixture = ROOT / "target/fixtures/rust-network-server.elf"
        self.runs = PipelineRuns(self.path, self.runner, self.fixture)

    async def asyncTearDown(self):
        await self.runs.close()
        self.temp.cleanup()

    async def test_completed_projection_replays_exact_engine_journal(self):
        state = self.runs.create("upload", "exit.elf", elf(b"\x31\xff\xb8\x3c\x00\x00\x00\x0f\x05"))
        await asyncio.wait_for(self.runs.tasks[state["id"]], 10)
        self.assertEqual(state["status"], "completed", state["message"])
        self.assertEqual(len(state["groups"]), 13)
        self.assertGreater(state["groups"]["audit"]["count"], 0)
        self.assertEqual(state["groups"]["audit"]["output"]["type"], "security")
        self.assertEqual(state["groups"]["audit"]["output"]["verdict"], "accepted")
        self.assertGreater(state["counters"]["rv64_steps"], 0)
        engine = [json.loads(s) for s in Path(state["ledger"]).read_text().splitlines()]
        copied = [json.loads(row[0]) for row in self.runs.db.execute("SELECT body FROM events WHERE run_id=? ORDER BY seq", (state["id"],))]
        self.assertEqual(engine, copied)
        self.assertEqual(sum(g["count"] for g in state["groups"].values()), len(engine))
        for event in engine:
            self.runs.append(state["id"], event)
        self.assertEqual(state["cursor"], len(engine))
        await self.runs.close()
        self.runs = PipelineRuns(self.path, self.runner, self.fixture)
        self.assertEqual(self.runs.snapshot(state["id"]), state)

    async def test_failure_is_at_actual_group_with_unreached_groups_empty(self):
        for code, group in [(b"\x0f\x0b", "lower"), (b"\xcd\x80", "audit"), (None, "load")]:
            state = self.runs.create("upload", "invalid.elf", elf(code) if code else b"not an ELF")
            await asyncio.wait_for(self.runs.tasks[state["id"]], 10)
            self.assertEqual(state["status"], "failed", state)
            self.assertGreater(state["groups"][group]["failed"], 0)
            self.assertEqual(state["groups"]["codegen"]["count"], 0)
            self.assertEqual(state["engine_result"]["status"], "failed")

    async def test_cancel_reaps_process_and_does_not_manufacture_success(self):
        state = self.runs.create("upload", "loop.elf", elf(b"\xeb\xfe"))
        for _ in range(300):
            if state["counters"].get("rv64_steps", 0) > 0:
                break
            await asyncio.sleep(.01)
        self.assertGreater(state["counters"].get("rv64_steps", 0), 0)
        with self.assertRaisesRegex(RuntimeError, "active"):
            self.runs.create("upload", "second.elf", elf(b"\xeb\xfe"))
        await self.runs.cancel(state["id"])
        await asyncio.wait_for(self.runs.tasks[state["id"]], 5)
        self.assertEqual(state["status"], "cancelled")
        self.assertIsNone(state["engine_result"])
        self.assertGreater(state["groups"]["execute"]["active"], 0)
        self.assertEqual(self.runs.processes, {})

    async def test_coordinator_shutdown_marks_interruption(self):
        state = self.runs.create("upload", "loop.elf", elf(b"\xeb\xfe"))
        for _ in range(300):
            if state["cursor"] > 15:
                break
            await asyncio.sleep(.01)
        await self.runs.close()
        self.runs = PipelineRuns(self.path, self.runner, self.fixture)
        self.assertEqual(self.runs.snapshot(state["id"])["status"], "interrupted")
        self.assertIsNone(self.runs.snapshot(state["id"])["engine_result"])

    async def test_dropped_engine_transport_resumes_live_from_committed_cursor(self):
        requests, peers = [], set()
        endpoint = None

        async def proxy(reader, writer):
            task = asyncio.current_task()
            peers.add(task)
            upstream = None
            try:
                head = await reader.readuntil(b"\r\n\r\n")
                requests.append(head.split(b"\r\n", 1)[0])
                source, upstream = await asyncio.open_connection(endpoint.hostname, endpoint.port)
                upstream.write(head)
                await upstream.drain()
                # Cut the real TCP transport after the first engine record.
                # Every later connection forwards the actual runner unchanged.
                cut = len(requests) == 1
                while line := await source.readline():
                    writer.write(line)
                    await writer.drain()
                    if cut and line.startswith(b"data: "):
                        break
            finally:
                for socket in [upstream, writer]:
                    if socket:
                        socket.close()
                        try:
                            await socket.wait_closed()
                        except OSError:
                            # The first transport is deliberately severed.
                            # A reset must not prevent closing the other half.
                            pass
                peers.discard(task)

        server = await asyncio.start_server(proxy, "127.0.0.1", 0)
        port = server.sockets[0].getsockname()[1]
        subscribe = self.runs._subscribe

        async def redirected(run_id, url):
            nonlocal endpoint
            endpoint = urlsplit(url)
            await subscribe(run_id, f"http://127.0.0.1:{port}/events")

        self.runs._subscribe = redirected
        try:
            state = self.runs.create("upload", "loop.elf", elf(b"\xeb\xfe"))
            # Durable journal fsyncs can contend with other acceptance probes.
            # Still require a live resumed stream, never terminal disk replay.
            for _ in range(1500):
                if len(requests) >= 2 and state["cursor"] > 20:
                    break
                if state["status"] in {"completed", "failed", "cancelled", "interrupted"}:
                    break
                await asyncio.sleep(.01)
            self.assertEqual(requests[:2], [b"GET /events?after=0 HTTP/1.1", b"GET /events?after=1 HTTP/1.1"])
            self.assertGreater(state["cursor"], 20)
            self.assertEqual(state["status"], "running")
            self.assertEqual(state["engine_connection"], "live")
            self.assertIsNone(state["engine_result"])
            await self.runs.cancel(state["id"])
            await asyncio.wait_for(self.runs.tasks[state["id"]], 5)
            engine = [json.loads(s) for s in Path(state["ledger"]).read_text().splitlines()]
            copied = [json.loads(row[0]) for row in self.runs.db.execute("SELECT body FROM events WHERE run_id=? ORDER BY seq", (state["id"],))]
            self.assertEqual(engine, copied)
        finally:
            server.close()
            await server.wait_closed()
            for task in list(peers):
                task.cancel()
            await asyncio.gather(*peers, return_exceptions=True)
