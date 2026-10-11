"""Acceptance-test consumer for the public SSE API; no engine internals."""
import http.client
import json
import socket
import threading
import time
from urllib.parse import urlsplit


def endpoint_from(stderr_path, process):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        for line in stderr_path.read_text().splitlines():
            if line.startswith("events: "):
                return line[len("events: "):]
        assert process.poll() is None, "runner exited before publishing live endpoint"
        time.sleep(0.005)
    raise AssertionError("runner did not publish a live endpoint")


def stalled_consumer(endpoint):
    url = urlsplit(endpoint)
    stream = socket.socket()
    stream.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024)
    stream.settimeout(5)
    stream.connect((url.hostname, url.port))
    stream.sendall(f"GET /events HTTP/1.1\r\nHost: {url.netloc}\r\n\r\n".encode())
    # Intentionally never read the response, including its HTTP headers.
    return stream


class LiveConsumer:
    def __init__(self, endpoint, process, output, reconnect_after=20):
        self.endpoint, self.process, self.output = endpoint, process, output
        self.reconnect_after = reconnect_after
        self.events, self.receipts = [], []
        self.reconnections = 0
        self.error = None
        self.first_compile = threading.Event()
        self.thread = threading.Thread(target=self._run, daemon=True)
        self.thread.start()

    def _run(self):
        url = urlsplit(self.endpoint)
        try:
            with self.output.open("w") as capture:
                while True:
                    connection = http.client.HTTPConnection(url.hostname, url.port, timeout=30)
                    headers = {"Last-Event-ID": str(len(self.events))} if self.events else {}
                    response = None
                    try:
                        connection.request("GET", "/events?after=0", headers=headers)
                        response = connection.getresponse()
                        assert response.status == 200, response.status
                        assert response.getheader("Content-Type") == "text/event-stream"
                        event_id, kind, data = None, None, None
                        while True:
                            line = response.readline()
                            assert line, "SSE closed without a terminal result"
                            if line.startswith(b"id: "):
                                event_id = int(line[4:])
                            elif line.startswith(b"event: "):
                                kind = line[7:].strip()
                            elif line.startswith(b"data: "):
                                data = json.loads(line[6:])
                            elif line == b"\n" and data is not None:
                                assert kind == b"ledger" and event_id == data["seq"]
                                assert data["seq"] == len(self.events) + 1, "gap or duplicate on resume"
                                receipt = {"received_unix_ns": str(time.time_ns()),
                                           "runner_alive": self.process.poll() is None, "event": data}
                                capture.write(json.dumps(receipt) + "\n")
                                capture.flush()
                                self.events.append(data)
                                self.receipts.append(receipt)
                                if data["type"] == "compilation_progress":
                                    self.first_compile.set()
                                if data["type"] == "result":
                                    return
                                if len(self.events) == self.reconnect_after and not self.reconnections:
                                    self.reconnections += 1
                                    break
                                event_id, kind, data = None, None, None
                    finally:
                        if response is not None:
                            response.close()
                        connection.close()
        except Exception as error:
            self.error = f"{type(error).__name__}: {error}"

    def finish(self, journal_events):
        self.thread.join(timeout=10)
        assert not self.thread.is_alive(), "live consumer did not finish"
        assert self.error is None, self.error
        assert self.events == journal_events, "SSE payloads differ from committed journal"
        assert self.reconnections == 1
        compiled = [r for r in self.receipts if r["event"]["type"] == "compilation_progress"]
        assert len(compiled) > 1 and all(r["runner_alive"] for r in compiled)
        terminal_time = int(journal_events[-1]["timestamp_unix_ns"])
        before_result = [r for r in compiled if int(r["received_unix_ns"]) < terminal_time]
        assert len(before_result) > 1, "compilation was only observed after completion"
        catalog = journal_events[0]["groups"]
        groups = {g["id"]: g["order"] for g in catalog}
        assert len(groups) == 13
        assert all(groups[e["group"]] == e["group_order"] for e in journal_events)
        return {"status": "passed", "endpoint": self.endpoint, "capture": str(self.output),
                "events_received": len(self.events), "reconnections": self.reconnections,
                "gaps": 0, "duplicates": 0, "matches_durable_journal": True,
                "compilation_updates_before_result": len(before_result),
                "first_compilation_received_unix_ns": before_result[0]["received_unix_ns"],
                "last_compilation_received_unix_ns": before_result[-1]["received_unix_ns"],
                "groups": catalog}
