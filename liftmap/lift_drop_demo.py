#!/usr/bin/env python3
"""
lift_drop_demo.py -- lift every function the ingest tracer selects.

Reads lift_map.json and, for each LIFT decision that is a single remill trace
(no callees), builds an Anvill spec, runs anvill-decompile-spec, and checks the
result on the Pulley interpreter and on qemu-riscv64.

  * ingest_tracer  -> decides WHERE to lift or keep native
  * pipeline/lift_drop.py -> anvill spec -> anvill-decompile-spec -> Cranelift/Pulley and qemu
"""
import os
import sys
import json
import argparse
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
sys.path.insert(0, HERE)
import placement  # noqa: E402  (keyed lift placement)

OBJ = os.path.join(HERE, "sample_network.o")
O2PULLEY = os.path.join(REPO, "ceremony", "o2pulley.sh")
JSON = os.path.join(HERE, "lift_map.json")
PLAN = os.path.join(HERE, "lift_plan.json")
RESULTS = os.path.join(HERE, "lift_results.json")

# Reference semantics for the sample. Names are compared with the leading
# underscore stripped so the same cases work for ELF and Mach-O symbols.
def _ip_id_hash(a, b):
    h = ((a ^ b) * 2654435761) & 0xFFFFFFFF
    return ((h >> 16) ^ (h & 0xFFFF)) & 0xFFFFFFFF


CASES = {
    "tcp_window_scaled": {
        "arguments": [100, 50],
        "expected": 150,
    },
    "ip_id_hash": {
        "arguments": [100, 50],
        "expected": _ip_id_hash(100, 50),
    },
    "parse_packet": {
        "arguments": [
            ("mem", bytes([0x45, 0, 0, 0x14] + [0] * 16)),
            20,
            ("out", "u32"),
            ("out", "u32"),
        ],
        "expected": 89,
        "expected_outs": [4, 20],
    },
}


def ensure_sample(path):
    """Build sample_network[.o] from the committed sample_network.c (x86_64)."""
    if os.path.exists(path):
        return True
    import subprocess
    here = os.path.dirname(os.path.abspath(__file__))
    c = os.path.join(here, "sample_network.c")
    if not os.path.exists(c):
        return False
    cc = os.environ.get("CC", "cc")
    flags = ["-O1", "-fno-inline"]
    if sys.platform == "darwin":
        flags += ["-arch", "x86_64"]
    cmd = [cc] + flags + (["-c", c, "-o", path] if path.endswith(".o")
                          else [c, "-o", path])
    try:
        subprocess.run(cmd, check=True, capture_output=True)
        return os.path.exists(path)
    except Exception as e:  # pragma: no cover
        sys.stderr.write(f"  warn: failed to build sample ({e})\n")
        return False


def norm_name(name):
    return name.lstrip("_")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--plan-only", action="store_true",
                    help="print the lift plan (exact offsets/signature/metrics) "
                         "and guardrail verdict without running remill/LLVM")
    ns = ap.parse_args()

    # Always re-analyze so a stale lift_map.json cannot disagree with the object.
    r = subprocess.run(
        [sys.executable, os.path.join(HERE, "ingest_tracer.py"),
         OBJ, JSON],
        capture_output=True, text=True,
    )
    if r.returncode != 0 or not os.path.exists(JSON):
        print("failed to run ingest_tracer.py:", file=sys.stderr)
        print(r.stderr.strip(), file=sys.stderr)
        return 2
    if not os.path.exists(OBJ):
        ensure_sample(OBJ)
    report = json.load(open(JSON))

    # Final placement. The tracer decided which functions are ELIGIBLE
    # (performance-safe); placement decides which of those actually move, using
    # a per-deployment key when one is configured. No key -> deterministic
    # (every eligible function), which is what CI uses.
    lift_key = placement.load_key()
    lift_plan = placement.plan(report, lift_key)
    with open(PLAN, "w") as plan_fh:
        json.dump(lift_plan, plan_fh, indent=2)
    selected_norms = {norm_name(n) for n in lift_plan["selected"]}

    print()
    print(f"  PLACEMENT  : mode={lift_plan['mode']} "
          f"(key {lift_plan.get('key_fingerprint', 'none')})")
    print(f"    selected     : {lift_plan['selected']}")
    print(f"    held NATIVE  : {lift_plan['held_native']}")

    print("=" * 74)
    print(" MULTI LIFT-AND-DROP  --  ingest_tracer analysis of",
          os.path.basename(report["binary"]))
    print("=" * 74)
    print(f"  functions analyzed : {report['total_functions']}")
    hdr = f"  {'function':22} {'decision':11} {'net':>4} {'freq':>4} {'loop':>5} {'pure':>5} {'sig':>14}"
    print(hdr)
    print("  " + "-" * 70)
    for f in sorted(report["functions"], key=lambda x: (x["decision"] != "LIFT", -x["lift_score"])):
        print(f"  {f['name']:22} {f['decision']:11} {f['network_score']:>4} "
              f"{f['frequency_score']:>4} {f['loop_density']:>5} "
              f"{str(f['pure_compute']):>5} {f['signature']:>14}")
    print()
    print("  LIFT        :", report["lift"])
    print("  KEEP_NATIVE :", report["keep_native"])
    print()
    print("  WHY:")
    for f in report["functions"]:
        print(f"    - {f['name']:22} -> {f['decision']}: {f['reason']}")

    sys.path.insert(0, os.path.join(REPO, "pipeline"))
    from lift_drop import lift_and_drop, LOOP_DENSITY_REJECT, CALL_FRACTION_REJECT

    lifted = []
    results = []
    for f in sorted(report["functions"], key=lambda x: -x["lift_score"]):
        if f["decision"] != "LIFT" or not f.get("liftable_now"):
            continue
        if norm_name(f["name"]) not in selected_norms:
            continue  # keyed placement kept this one native
        key = norm_name(f["name"])
        case = CASES.get(key)
        if case is None:
            print(f"\n  ERROR: no validation case for lift candidate {f['name']}",
                  file=sys.stderr)
            return 1
        metrics = {
            "loop_density": f["loop_density"],
            "call_fraction": f["call_fraction"],
        }
        # Guardrail verdict the lift op will enforce (defense-in-depth). The
        # tracer already excluded these, but we show the check explicitly.
        if (f["loop_density"] > LOOP_DENSITY_REJECT
                or f["call_fraction"] > CALL_FRACTION_REJECT):
            verdict = "REJECT (would refuse to lift)"
        else:
            verdict = "OK"

        if ns.plan_only:
            # Show the exact hand-off the ingest tracer makes to the lift op:
            # the authoritative byte window (addr/size), the recovered ABI
            # signature, and the performance metrics the lift op re-checks.
            print()
            print("=" * 74)
            print(f" PLAN  --  {f['name']}  (no lift; --plan-only)")
            print("=" * 74)
            print(f"  reason     : {f['reason']}")
            print(f"  addr       : 0x{f['addr']:x}  (tracer-authoritative start)")
            print(f"  size_bytes : {f['size_bytes']}  (tracer-authoritative extent)")
            print(f"  signature  : {f['signature']}  (tracer-recovered ABI)")
            print(f"  metrics    : loop_density={f['loop_density']} "
                  f"call_fraction={f['call_fraction']}")
            print(f"  guardrail  : {verdict}")
            lifted.append(f["name"])
            continue

        print()
        print("=" * 74)
        print(f" LIFT+RUN  --  {f['name']}  {f['signature']}  "
              f"[{f['addr']:#x}, +{f['size_bytes']}]")
        print("=" * 74)
        print(f"  {f['reason']}")
        try:
            res = lift_and_drop(
                obj=OBJ,
                hexbytes=None,
                symbol=f["name"],
                signature=f["signature"],
                arguments=case["arguments"],
                expected=case["expected"],
                expected_outs=case.get("expected_outs"),
                qemu=True,
                addr=f["addr"],
                size=f["size_bytes"],
                metrics=metrics,
            )
        except SystemExit as exc:
            print(f"  VALIDATION : FAIL  ({f['name']})", file=sys.stderr)
            return int(exc.code or 1)
        print(f"  VALIDATION : PASS  ({f['name']} == {case['expected']})")
        lifted.append(f["name"])
        results.append({
            "name": f["name"],
            "addr": f["addr"],
            "size_bytes": f["size_bytes"],
            "signature": f["signature"],
            "loop_density": f["loop_density"],
            "call_fraction": f["call_fraction"],
            "network_score": f["network_score"],
            "reason": f["reason"],
            "hexbytes": res.get("hexbytes", ""),
            "expected": case["expected"],
            "expected_outs": case.get("expected_outs"),
            "pulley": res.get("pulley", ""),
            "qemu": res.get("qemu", ""),
            "passed": True,
        })

    if not lifted:
        print("\n  ERROR: ingest tracer selected nothing to lift", file=sys.stderr)
        return 1

    if ns.plan_only:
        print()
        print("  PLAN SUMMARY (deterministic performance-safe placement; the")
        print("  attacker would need the deployed binary to recompute this):")
        print(f"    * lift plan            : {lifted}")
        print(f"    * lift-eligible        : {report['lift']}")
        print(f"    * kept NATIVE          : {report['keep_native']}")
        return 0

    print()
    print("  MULTI LIFT-AND-DROP SUMMARY:")
    print(f"    * lifted and validated : {lifted}")
    print(f"    * lift-eligible        : {report['lift']}")
    print(f"    * kept NATIVE          : {report['keep_native']}")

    # Machine-readable outcome for liftmap/visualize.py.
    with open(RESULTS, "w") as res_fh:
        json.dump(
            {
                "binary": report.get("binary"),
                "placement": lift_plan,
                "results": results,
                "keep_native": report.get("keep_native", []),
            },
            res_fh,
            indent=2,
        )
    print(f"    * wrote {os.path.relpath(RESULTS, REPO)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
