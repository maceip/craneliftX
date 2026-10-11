"""Supervisor failure tests: real subprocesses and durable SQLite, no fake success flags."""

import asyncio
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from server import Runs, TIERS

START = {
    "v": 1,
    "type": "started",
    "tier": "baby",
    "total_bytes": 64,
    "backend": "riscv64gc-unknown-none-elf",
    "seed": "42",
    "connections": 0,
    "kernels": 1,
    "rounds": 1,
    "oracle": "native-x86-and-rust-reference",
}
SECURITY = {
    "v": 1,
    "type": "security",
    "syscall_rejected": True,
    "codegen_invoked": False,
    "stage": "audit",
}
RESULT = {
    "v": 1,
    "type": "result",
    "bytes": 64,
    "verified": 1,
    "mismatches": 0,
    "syscall_rejected": True,
    "trace_executions": 1,
    "riscv_steps": 74,
    "execution_ns": 100,
    "compilation_ns": 200,
    "oracle": "native-x86-and-rust-reference",
}


def lines(*events):
    return "".join(
        f"print({json.dumps(json.dumps(event))},flush=True)\n" for event in events
    )


class Supervisor(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        self.managers = []

    async def asyncTearDown(self):
        for manager in self.managers:
            await manager.close()
        self.tmp.cleanup()

    def manager(self, source):
        worker = self.root / f"worker-{len(self.managers)}"
        worker.write_text(f"#!{sys.executable}\nimport sys,time\n{source}")
        worker.chmod(0o755)
        manager = Runs(self.root / f"state-{len(self.managers)}", worker)
        self.managers.append(manager)
        return manager

    async def execute(self, source):
        manager = self.manager(source)
        run = manager.create("baby", "42")
        await asyncio.wait_for(manager.tasks[run["id"]], 10)
        return manager.snapshot(run["id"])

    async def test_result_requires_clean_exit(self):
        for source in [
            lines(START, SECURITY, RESULT) + "sys.exit(3)\n",
            lines(START, SECURITY),
            lines(START, SECURITY, {**RESULT, "verified": 0}),
            lines(START, RESULT),
            lines(START, SECURITY, RESULT, RESULT),
        ]:
            state = await self.execute(source)
            self.assertEqual(state["status"], "failed")
            self.assertNotIn("result", state)

    async def test_drains_both_pipes_and_bounds_diagnostics(self):
        source = (
            lines(START)
            + 'for i in range(1000): print("x"*1000,file=sys.stderr,flush=True)\n'
            + lines(SECURITY, RESULT)
        )
        state = await self.execute(source)
        self.assertEqual(state["status"], "completed")
        self.assertLessEqual(len(state["logs"]), 80)

    async def test_malformed_protocol_fails_and_reaps_process(self):
        state = await self.execute('print("not-json",flush=True)\ntime.sleep(30)\n')
        self.assertEqual(state["status"], "failed")
        self.assertEqual(self.managers[-1].processes, {})

    async def test_cancellation_force_kills_uncooperative_worker(self):
        manager = self.manager(lines(START) + "time.sleep(60)\n")
        state = manager.create("baby", "42")
        while state["id"] not in manager.processes:
            await asyncio.sleep(0.01)
        await manager.stop(state["id"])
        await asyncio.wait_for(manager.tasks[state["id"]], 5)
        self.assertEqual(manager.snapshot(state["id"])["status"], "cancelled")
        self.assertEqual(manager.processes, {})

    async def test_single_run_and_durable_restart(self):
        manager = self.manager(lines(START) + "time.sleep(60)\n")
        state = manager.create("baby", "42")
        self.assertIsNone(manager.create("baby", "42"))
        await asyncio.sleep(0.03)
        # Simulate an incomplete durable record at startup using a second state path.
        other = self.manager(lines(START, SECURITY, RESULT))
        other.db.execute(
            "INSERT INTO runs VALUES(?,?,?)",
            (state["id"], json.dumps(state), state["created"]),
        )
        other.db.commit()
        path = self.root / "state-1"
        await other.close()
        self.managers.remove(other)
        reopened = Runs(path, manager.worker)
        self.managers.append(reopened)
        recovered = reopened.snapshot(state["id"])
        self.assertEqual(recovered["status"], "interrupted")
        rows = reopened.db.execute(
            "SELECT seq FROM events WHERE run_id=?", (state["id"],)
        ).fetchall()
        self.assertEqual(rows, [(state["cursor"] + 1,)])

    async def test_actual_worker_baby(self):
        worker = Path(
            os.environ.get(
                "ISR_WORKER",
                Path(__file__).resolve().parents[2] / "target/release/isr-demo",
            )
        )
        manager = Runs(self.root / "real", worker)
        self.managers.append(manager)
        state = manager.create("baby", "42")
        await asyncio.wait_for(manager.tasks[state["id"]], 10)
        state = manager.snapshot(state["id"])
        self.assertEqual(state["status"], "completed", state.get("message"))
        self.assertEqual(state["result"]["verified"], 1)
        self.assertEqual(len(state["artifacts"]), 1)
        self.assertTrue(state["context"]["rip_after"].startswith("0x"))
        self.assertTrue(state["engine"]["timestamp_unix_ns"].isdigit())
        artifact = state["artifacts"]["0"]
        self.assertEqual(artifact["decoded_instructions"], artifact["instructions"] + 1)
        self.assertEqual(state["result"]["unique_lifted_bytes"], 64)
        self.assertEqual(state["result"]["compiled_traces"], 1)
        self.assertEqual(state["security"]["events"][-1]["state"], "failed")
        self.assertEqual(state["security"]["events"][-1]["stage"], "audit")
        events = [json.loads(row[0]) for row in manager.db.execute(
            "SELECT body FROM events WHERE run_id=? ORDER BY seq", (state["id"],)
        )]
        worker_events = [event for event in events if "worker_seq" in event]
        self.assertEqual([event["worker_seq"] for event in worker_events],
                         list(range(1, len(worker_events) + 1)))
        self.assertEqual(len({event["worker_run_id"] for event in worker_events}), 1)


if __name__ == "__main__":
    unittest.main()
