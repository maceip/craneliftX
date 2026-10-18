# ISR Server Harness

Production deployment guide for running hardened x86-64 guest binaries on Linux servers.

## Overview

The ISR Server Harness (`isr-server`) is a supervisor daemon that loads a static x86-64 ELF binary, runs it under the ISR hardened runtime, and provides an HTTP admin endpoint for health checks and Prometheus metrics.

**What the harness does:**
- Loads a guest binary into an isolated address space
- Intercepts all syscalls through the host bridge
- Enforces heap mitigations (shadow tags, PAC, randomized allocator) on the
  interpreted path
- Rejects transfers into the middle of a known guest instruction (CFI)
- Compiles lifted code with register diversification (x86_64 backend)
- JIT pages are W→X transitioned (never writable+executable)
- Restarts the guest on crash with configurable backoff
- Exposes `/health` and `/metrics` endpoints

**What the guest binary sees:**
- A standard Linux x86-64 environment with loopback TCP networking
- `malloc`/`free`/`calloc`/`realloc` are transparently replaced with the hardened allocator
- On the `rv64` backend, all memory accesses are checked against the shadow tag map

**Production backend:** `rv64` (default). Heap tags, PAC and the mid-instruction
CFI check are enforced there. The `x86_64` native backend is experimental: it
runs W^X + diversification only, and raw native loads/stores bypass `ProcessBus`
so heap tags and PAC do not apply. It requires `allow_experimental_native = true`.

## Quick Start

### 1. Build the harness

```bash
cargo build --release --bin isr-server
sudo cp target/release/isr-server /usr/local/bin/
```

### 2. Prepare your guest binary

Your binary **must be a statically linked x86-64 ELF**.

```bash
# C/C++
gcc -static -O2 -o /opt/isr/guest-server server.c

# Rust
RUSTFLAGS='-C target-feature=+crt-static' \
  cargo build --release --target x86_64-unknown-linux-gnu

# Go
CGO_ENABLED=0 go build -ldflags '-extldflags -static'
```

If you have a dynamic binary, `scripts/static-link.sh` will analyze it and guide you through conversion.

### 3. Create a config file

```bash
sudo mkdir -p /etc/isr
sudo cp deploy/isr-server.toml /etc/isr/guest.toml
# Edit /etc/isr/guest.toml and set guest.path
```

### 4. Run

```bash
# One-shot (for testing)
isr-server /etc/isr/guest.toml --once

# Daemon mode with auto-restart
isr-server /etc/isr/guest.toml
```

### 5. Health check

```bash
curl http://127.0.0.1:8080/health
curl http://127.0.0.1:8080/metrics
```

## Systemd Deployment

```bash
sudo cp deploy/isr-server.service /etc/systemd/system/
sudo useradd -r -s /sbin/nologin isr || true
sudo mkdir -p /var/log/isr-server
sudo chown isr:isr /var/log/isr-server
sudo systemctl daemon-reload
sudo systemctl enable --now isr-server
sudo systemctl status isr-server
```

## Docker Deployment

```bash
docker build -f deploy/Dockerfile -t isr-server:latest .
docker run -d \
  -p 8080:8080 \
  -v $(pwd)/deploy/isr-server.toml:/etc/isr/guest.toml:ro \
  -v $(pwd)/guest-binary:/opt/isr/guest-server:ro \
  -v isr-logs:/var/log/isr-server \
  --name isr-server \
  isr-server:latest
```

## Configuration Reference

| Section | Key | Default | Description |
|---------|-----|---------|-------------|
| `guest` | `path` | — | Path to static x86-64 ELF |
| `execution` | `backend` | `rv64` | `rv64` (hardened, default) or `x86_64` (experimental) |
| `execution` | `allow_experimental_native` | false | Required for `backend = "x86_64"` |
| `execution` | `max_seconds` | 300 | Per-run timeout |
| `hardening` | `tags` | true | 16-byte shadow heap tagging |
| `hardening` | `pac` | true | Pointer authentication codes |
| `hardening` | `random_placement` | true | CSPRNG heap slot selection |
| `hardening` | `cfi` | true | Reject mid-instruction transfer targets |
| `server` | `admin_bind` | `0.0.0.0:8080` | Health/metrics HTTP endpoint |
| `restart` | `policy` | `always` | `always` / `on-failure` / `never` |
| `restart` | `backoff_seconds` | 5 | Delay between restarts |
| `logging` | `ledger_dir` | `/var/log/isr-server` | Durable JSONL events |

## What Works with Arbitrary Binaries

### Fully Supported (no source needed)
- **Static x86-64 ELF executables** — any compiler, any language
- **Stripped binaries** — symbols are not required; we disassemble by address
- **Standard Linux syscalls** — read, write, socket, bind, listen, accept, close, exit
- **mmap/munmap/mprotect** — for allocators, thread stacks, JIT buffers
- **brk** — legacy allocator path
- **getrandom/clock_gettime/getpid/getuid** — common runtime services
- **Custom allocators** — intercepted via `malloc`/`free` symbol scanning or syscall-level `brk`/`mmap`

### Not Yet Supported
- **Dynamically linked binaries** — PT_INTERP/PT_DYNAMIC segments are rejected
- **Threads** — `clone`/`futex` not implemented; runtime is single-threaded
- **Signals** — `sigaction`/`kill` not implemented
- **Non-loopback bind** — currently loopback-only (configurable in future)
- **File I/O** — no `open`/`read`/`write` for filesystem descriptors yet

### Mitigations That Work Regardless of Source

| Mitigation | Works on Arbitrary Binary? | Mechanism |
|------------|---------------------------|-----------|
| Shadow heap tags | **Yes (rv64)** | Every load/store in the interpreter goes through `ProcessBus::screen()` |
| Pointer authentication | **Yes (rv64)** | All code pointers in writable memory are signed/verified |
| Randomized heap | **Yes** | `malloc`/`free` are intercepted by symbol scan or syscall fallback |
| Mid-instruction CFI | **Yes** | Dispatch target must not fall inside a known instruction |
| Register diversification | **x86_64 only** | Per-compilation random mapping in Cranelift backend |
| W^X JIT pages | **x86_64 only** | `mmap` → `mprotect(PROT_READ|PROT_EXEC)` on all compiled code |

The x86_64 native backend does **not** run loads/stores through `ProcessBus`,
so heap tags and PAC are not enforced there.

## Architecture

```
┌─────────────────────────────────────────┐
│           isr-server daemon             │
│  ┌─────────────────────────────────┐    │
│  │   Admin HTTP (health/metrics)   │    │
│  └─────────────────────────────────┘    │
│  ┌─────────────────────────────────┐    │
│  │   Supervisor (restart, backoff) │    │
│  └─────────────────────────────────┘    │
│  ┌─────────────────────────────────┐    │
│  │   Guest binary (static ELF)     │    │
│  │   ┌─────────────────────────┐   │    │
│  │   │  Lifted functions       │   │    │
│  │   │  ┌─────────────────┐    │   │    │
│  │   │  │ Cranelift x86_64 │    │   │    │
│  │   │  │ + diversification │    │   │    │
│  │   │  │ + inline CFI      │    │   │    │
│  │   │  │ + W^X pages       │    │   │    │
│  │   │  └─────────────────┘    │   │    │
│  │   └─────────────────────────┘   │    │
│  │   ┌─────────────────────────┐   │    │
│  │   │  Native functions       │   │    │
│  │   │  (entry, loops, glue)   │   │    │
│  │   └─────────────────────────┘   │    │
│  │   ┌─────────────────────────┐   │    │
│  │   │  Hardened Heap          │   │    │
│  │   │  (tags, PAC, random)    │   │    │
│  │   └─────────────────────────┘   │    │
│  └─────────────────────────────────┘    │
│  ┌─────────────────────────────────┐    │
│  │   Syscall Bridge                │    │
│  │   (socket, bind, accept, read,  │    │
│  │    write, mmap, brk, ...)       │    │
│  └─────────────────────────────────┘    │
└─────────────────────────────────────────┘
```

## Security Model

The harness protects against memory-safety exploits in the guest binary:

1. **Heap overflow** → detected by shadow tag mismatch on access
2. **Use-after-free** → detected by quarantine tag (0xFF) on access
3. **Pointer substitution** → detected by PAC verification failure
4. **ROP/JOP** → blocked by inline CFI bitmap + diversified gadget sets
5. **Code injection** → blocked by W^X JIT pages + non-executable heap

The attacker cannot bypass these mitigations by reading memory because:
- The tag map and PAC secret are Rust host values, never in guest address space
- The CSPRNG seed is drawn from `/dev/urandom` at startup
- The JIT code pages are RX-only after compilation

## Troubleshooting

### Guest exits immediately with "dynamic linking not supported"
Your binary is dynamically linked. Rebuild with `-static` or use the language-specific instructions above.

### Guest crashes with "unsupported Linux x86-64 syscall N"
The syscall bridge does not yet implement that syscall. Check the syscall number against `src/process/syscall.rs`. Common missing syscalls for servers are `openat`, `futex`, `clone`. File an issue with the syscall number and your use case.

### Admin endpoint returns "unhealthy"
The guest has crashed or exited. Check `/var/log/isr-server/events.jsonl` for the `result` event with error details.

### High restart count
Check `/metrics` for `isr_server_restarts`. If restarts are frequent, the guest may be hitting an unsupported syscall or a mitigation fault. Review the ledger for `Mitigation` errors.

## Future Work

- Multi-threading: implement `clone`/`futex`/`exit_group`
- Dynamic linking: lift shared libraries on demand
- File I/O: implement `openat`/`read`/`write` for filesystem access
- Seccomp-bpf: sandbox the harness itself
- eBPF integration: offload tag checks to kernel
