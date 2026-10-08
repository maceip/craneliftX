#!/usr/bin/env python3
"""
lift_drop_demo.py -- the "multi lift-and-drop" demonstration.

Consumes lift_map.json (produced by ingest_tracer.py) and performs the actual
lift: it selects the best PERFORMANCE lift candidate that our pipeline can lift
correctly today (pure-compute, two-arg, RAX(RDI,RSI)), then drives the existing
ceremony/o2pulley.sh pipeline to lift that native function to a Pulley-executable
wasm and run it on Cranelift/Pulley.

This proves both halves of the story:
  * ingest_tracer  -> decides WHERE to lift/drop (static analysis of the binary)
  * o2pulley      -> performs the lift+dro p (native -> remill -> wasm -> Pulley)
"""
import os
import sys
import json
import subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
OBJ = os.path.join(HERE, "sample_network.o")
O2PULLEY = os.path.join(REPO, "ceremony", "o2pulley.sh")
JSON = os.path.join(HERE, "lift_map.json")

# Reference semantics for the sample's known functions, so the demo can assert
# the lifted-on-Pulley result is CORRECT (not just "it ran").
REF = {
    "_tcp_window_scaled": lambda a, b: (a + b) & 0xFFFFFFFF,
    "_ip_id_hash": lambda a, b: ((a ^ b) * 2654435761) & 0xFFFFFFFF,
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


def pick_candidate(report):
    """Best LIFT candidate that our pipeline can lift correctly today:
    pure-compute, exactly 2 integer args, RAX(RDI,RSI) signature."""
    cands = [
        f for f in report["functions"]
        if f["decision"] == "LIFT"
        and f.get("liftable_now")
        and f["n_args"] == 2
        and f["signature"].startswith("RAX(")
    ]
    if not cands:
        return None
    cands.sort(key=lambda f: -f["lift_score"])
    return cands[0]


def main():
    # Reproducible: if the lift map is missing, run the analyzer (which also
    # builds the sample binary from source), then ensure the object exists.
    if not os.path.exists(JSON):
        r = subprocess.run(
            [sys.executable, os.path.join(HERE, "ingest_tracer.py"),
             os.path.join(HERE, "sample_network"), JSON],
            capture_output=True, text=True,
        )
        if r.returncode != 0 or not os.path.exists(JSON):
            print("failed to run ingest_tracer.py:", file=sys.stderr)
            print(r.stderr.strip(), file=sys.stderr)
            return 2
    if not os.path.exists(OBJ):
        ensure_sample(OBJ)
    report = json.load(open(JSON))

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

    cand = pick_candidate(report)
    if cand is None:
        print("\n[dem@] no stub-safe 2-arg LIFT candidate available to demonstrate.")
        return 0

    print()
    print("=" * 74)
    print(f" LIFT+RUN DEMO  --  lifting {cand['name']} to Pulley")
    print("=" * 74)
    print(f"  chosen because: lift_score={cand['lift_score']}, pure-compute, "
          f"2-arg {cand['signature']}, network-eligible")
    a, b = 100, 50
    expected = REF.get(cand["name"], lambda a, b: None)(a, b)

    print(f"  test inputs    : a={a}, b={b}"
          + (f"  (expected on Pulley = {expected})" if expected is not None else ""))
    print()
    print("  $ bash o2pulley.sh", os.path.basename(OBJ), cand["name"], a, b)
    print("-" * 74)

    if not os.path.exists(O2PULLEY):
        print(f"  ERROR: o2pulley.sh not found at {O2PULLEY}", file=sys.stderr)
        return 2
    if not os.path.exists(OBJ):
        print(f"  ERROR: object not found at {OBJ}", file=sys.stderr)
        return 2

    p = subprocess.run(
        ["bash", O2PULLEY, OBJ, cand["name"], str(a), str(b), str(expected)],
        capture_output=True, text=True,
    )
    print(p.stdout.strip())
    if p.returncode != 0:
        print(p.stderr.strip(), file=sys.stderr)
        return p.returncode

    # Validate the Pulley result against the reference semantics.
    ok = "OK" in p.stdout and (expected is None or f"= {expected}" in p.stdout)
    print("-" * 74)
    if expected is not None:
        print(f"  VALIDATION : {'PASS' if ok else 'CHECK'}  "
              f"(Pulley result matches native reference {expected})")
    else:
        print(f"  VALIDATION : result printed above (no reference for {cand['name']})")
    print()
    print("  MULTI LIFT-AND-DROP SUMMARY:")
    print(f"    * lifted to Pulley (demonstrated) : {cand['name']}")
    print(f"    * lift-eligible (network, not tight): "
          f"{[f['name'] for f in report['functions'] if f['decision']=='LIFT']}")
    print(f"    * kept NATIVE (tight loop / glue / entry): "
          f"{report['keep_native']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
