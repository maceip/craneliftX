# Environment and dependency scope

**Purpose.** This is the authoritative map of everything the native -> Pulley
lift-and-drop pipeline needs, at what depth, and why. It exists so the
environment is provisioned **once, deliberately**, instead of being discovered
one compile error at a time.

Read this **before** touching a new machine or changing a toolchain version.
The pipeline is deep: it builds a C++ lifter (remill) and a decompiler stage
(anvill) against a pinned LLVM, plus a Rust interpreter host, plus a Python
analysis layer. Each layer has its own dependency depth, and the failure modes
are non-obvious.

The LLVM major is pinned in `.llvm-version` (currently **20**). Every tool name
in the repo derives from that file; `ci/check_llvm_version.sh` fails CI if any
reference drifts.

---

## 1. Layer map (what needs what, and how deep)

| Layer | Component | Needs | Depth / notes |
|---|---|---|---|
| 0 | Host toolchain | `cmake` (4.3.2), `ninja` (1.13.2), `pkg-config` | Build system for remill/anvill. |
| 0 | macOS SDK | Xcode CLT, `xcrun --show-sdk-path` | **Required on macOS.** llvm.org clang has no default sysroot. |
| 1 | **LLVM 20** | `llvm-config`, `clang`, `clang++`, `llc`, `llvm-link`, `opt`, `wasm-ld` **+ dev headers/libs/`lib/cmake/llvm`** | The deepest requirement. remill links LLVM as a library; anvill needs LLVM passes. Version pinned by `.llvm-version`. |
| 2 | Solver | `z3` (4.16.0) | anvill's configure requires `find_package(Z3 CONFIG)`; the script synthesizes a `Z3Config.cmake` from the plain library. |
| 2 | Protobuf **C++** | `libprotoc` 35.0 + **abseil** | anvill links `libprotobuf`; on macOS abseil must be linked explicitly. |
| 2 | remill vendored deps | XED, glog, gflags, gtest, **Sleigh** | Built by `vendor/remill/dependencies` superbuild into `build/remill-deps/install`. XED is built by its own `mfile.py`, not CMake. |
| 3 | Rust | `cargo` (1.95.0) | Builds `ceremony-wasm`, the Pulley interpreter runner. No LLVM needed. |
| 4 | Python | `uv` venv with `capstone` + `protobuf` | `ingest_tracer.py` disassembles with capstone; `anvill_spec.py` builds/parses the Anvill protobuf. |

**Two different protobufs** — this trips people up:
- the **C++** library (Homebrew/apt `protobuf`) that anvill *links*;
- the **Python** runtime (`pip`/`uv` `protobuf`) that `anvill_spec.py` *uses to
  write the `.pb` file* the C++ binary then reads. They only need to agree on
  the wire format, not the version.

---

## 2. How each platform gets LLVM 20

### Linux (CI, `docker/ci.Dockerfile`)
```sh
apt-get install clang-20 llvm-20-dev lld-20 libprotobuf-dev libz3-dev \
                cmake ninja-build pkg-config protobuf-compiler python3
```
Apt uses **suffixed** names (`llvm-config-20`, `clang-20`, `wasm-ld-20`) under
`/usr/lib/llvm-20/`. This is the layout the scripts were originally written for.

### macOS (dev machine)
Homebrew uses **unsuffixed** names under `/opt/homebrew/opt/llvm@20/bin`
(`llvm-config`, `clang`, ...). The scripts now handle both layouts:
`pipeline/build_lifters.sh` locates the pinned LLVM and writes shims to
`build/llvm-shims-<major>/bin` exposing **both** the suffixed names the rest of
the pipeline expects and the unsuffixed names remill's `BCCompiler.cmake`
probes for. `pipeline/lift_drop.py` prepends that shim dir to `PATH` at import.

Preferred: `brew install llvm@20`.
**But brew is blocked in some sandboxes** (`sandbox-exec: sandbox_apply:
Operation not permitted`). Fallback — official prebuilt, no brew, no sandbox:
```sh
curl -L -o /tmp/llvm20.tar.xz \
  https://github.com/llvm/llvm-project/releases/download/llvmorg-20.1.8/LLVM-20.1.8-macOS-ARM64.tar.xz
mkdir -p ~/llvm20 && tar -xf /tmp/llvm20.tar.xz -C ~/llvm20 --strip-components=1
```
(~1.4 GB download.) It ships `bin/`, headers, libs and `lib/cmake/llvm`, so
remill builds against it. Note: it provides `llvm-config` and `clang-20` but
**not** `llvm-config-20` / `clang++-20` — the shims supply those.

`pipeline/bootstrap_env.sh` does all of this idempotently.

---

## 3. Known macOS failure modes (all already fixed — do not re-derive)

| Symptom | Root cause | Fix (in `pipeline/build_lifters.sh`) |
|---|---|---|
| `fatal error: 'string.h' file not found` while building XED | llvm.org clang has no default sysroot | auto-detect `SDKROOT` via `xcrun --show-sdk-path` |
| XED `mfile.py install` exits 1 under ninja (but succeeds standalone) | `mfile.py` is not concurrency-safe | build the dependency superbuild serially (`-j 1`) |
| Same failure even when deps are already installed | XED's exit 1 is spurious; the artifacts are fine | `SKIP_DEPS=1` reuses `build/remill-deps/install` |
| `typeinfo for llvm::CallbackVH` undefined | prebuilt LLVM is built **without RTTI** | `-DCMAKE_CXX_FLAGS=-fno-rtti` for anvill |
| `absl::hash_internal::MixingHashState::kSeed` undefined | Homebrew `libprotobuf` needs abseil linked explicitly | link `absl_hash`, `absl_raw_hash_set`, `absl_hashtablez_sampler`, `absl_city`, `absl_base`, `absl_strings`, ... |
| `no LC_RPATH's found` / `@rpath/libunwind.1.dylib` | cmake strips the build rpath on install | `-DCMAKE_INSTALL_RPATH=$LLVM_PREFIX/lib`; for already-installed binaries: `install_name_tool -add_rpath <llvm>/lib <bin>` |

---

## 4. Provisioning from scratch (macOS, verified)

```sh
# 1. environment: LLVM 20 (brew, or prebuilt fallback) + uv venv
pipeline/bootstrap_env.sh

# 2. lifters: remill + anvill against LLVM 20
PATH=~/llvm20/bin:$PATH SKIP_DEPS=1 JOBS=$(sysctl -n hw.ncpu) make lifters

# 3. end-to-end lift + Pulley validation
make demo
```

`SKIP_DEPS=1` is only needed because of the XED quirk above; on a clean machine
you can drop it (the superbuild will run, serially, and takes longer).

`make demo` prefers `.venv/bin/python3` when present, so no manual `PATH`
juggling is needed once the venv exists.

---

## 5. Verification (what "working" looks like)

`make demo` must exit 0 and print:

```
_tcp_window_scaled(100, 50) = 150      (expected 150)
_ip_id_hash(100, 50)        = 51156    (expected 51156)
_parse_packet(...)          = 89       OUT u32 4, OUT u32 20

MULTI LIFT-AND-DROP SUMMARY:
  lifted and validated : ['_ip_id_hash', '_tcp_window_scaled', '_parse_packet']
  kept NATIVE          : ['_crc32_tight', '_handle_connection', '_main']
```

The three KEEP_NATIVE results are part of the contract: the ingest tracer must
reject `_crc32_tight` (tight loop, `loop_density` 0.517), `_handle_connection`
(orchestration fan-out) and `_main` (entry point). If any of those get lifted,
the performance guardrail is broken.

CI runs this as a hard gate on the x86_64 build (Linux image), and `publish`
needs it, so a broken lift blocks the release.
