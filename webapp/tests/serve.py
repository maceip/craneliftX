#!/usr/bin/env python3
"""Disposable real nginx + ASGI stack for browser checks, including crash recovery."""

import argparse
import json
import os
from pathlib import Path
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, default=8878)
args = parser.parse_args()
nginx = shutil.which("nginx")
if not nginx:
    raise SystemExit(
        "nginx is required: the browser tests exercise the real deployment proxy"
    )
with socket.socket() as reserve:
    reserve.bind(("127.0.0.1", 0))
    upstream = reserve.getsockname()[1]
temp = tempfile.TemporaryDirectory(prefix="isr-e2e-")
state = Path(temp.name)
config = (
    (ROOT / "deploy/nginx.conf")
    .read_text()
    .replace("listen 80;", f"listen 127.0.0.1:{args.port};")
    .replace("/var/www/craneliftx/webapp", str(ROOT / "webapp"))
    .replace("127.0.0.1:8765", f"127.0.0.1:{upstream}")
)
# Explicit local paths keep Ubuntu's system-owned logs/temp directories out of
# the unprivileged test process. MIME declarations are identical on both hosts.
temp_paths = "\n".join(
    f"{kind}_temp_path {state}/{kind};"
    for kind in ("client_body", "proxy", "fastcgi", "uwsgi", "scgi")
)
(state / "nginx.conf").write_text(
    f"pid {state}/nginx.pid;\nerror_log {state}/nginx.log;\nevents {{}}\nhttp {{\n"
    f"access_log off;\n{temp_paths}\n"
    "types { text/html html; text/css css; application/javascript js; font/woff2 woff2; }\n"
    f"{config}\n}}"
)
env = dict(os.environ, ISR_STATE_DIR=str(state / "journal"))
log = (state / "server.log").open("ab")
commands = {
    "server": [sys.executable, str(ROOT / "webapp/server.py"), "--port", str(upstream)],
    "proxy": [
        nginx,
        "-p",
        str(state) + "/",
        "-c",
        str(state / "nginx.conf"),
        "-g",
        "daemon off;",
    ],
}
processes = {}
stop = False
metadata = Path(tempfile.gettempdir()) / f"isr-e2e-stack-{args.port}.json"
control = state / "control"


def start(name):
    processes[name] = subprocess.Popen(
        commands[name], env=env, stdout=log, stderr=log, start_new_session=True
    )


def kill(name):
    process = processes.get(name)
    if process and process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=10)


def stopping(*_):
    global stop
    stop = True


signal.signal(signal.SIGTERM, stopping)
signal.signal(signal.SIGINT, stopping)
try:
    start("server")
    start("proxy")
    metadata.write_text(
        json.dumps({"control": str(control), "state": str(state), "upstream": upstream})
    )
    print(f"Real nginx demo stack: http://127.0.0.1:{args.port}", flush=True)
    while not stop:
        if control.exists():
            name = control.read_text().strip()
            control.unlink()
            if name in commands:
                kill(name)
                # A real outage: the browser must recover from connection failure.
                time.sleep(1)
                start(name)
        for name, process in processes.items():
            if process.poll() is not None:
                raise RuntimeError(
                    f"{name} exited: " + (state / "server.log").read_text()[-5000:]
                )
        time.sleep(0.1)
finally:
    for name in list(processes):
        kill(name)
    metadata.unlink(missing_ok=True)
    log.close()
    if os.environ.get("ISR_KEEP_E2E_STATE"):
        print(f"Evidence journal: {state}", flush=True)
        temp._finalizer.detach()
    else:
        temp.cleanup()
