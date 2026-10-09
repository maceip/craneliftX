#!/usr/bin/env python3
"""Keyed lift placement: decide WHICH eligible functions actually get lifted.

The ingest tracer (ingest_tracer.py) decides which functions are ELIGIBLE --
the performance-safe ones: not tight loops, not orchestration glue, not entry
points. This module decides which of those eligible functions are actually
moved onto the diverse ISA, and can make that choice depend on a per-deployment
secret.

Why this exists
---------------
When placement is purely a function of the binary, anyone who obtains the
deployed binary can recompute exactly which functions run under which ISA, and
aim a payload at the ones they can predict. Making the final selection depend
on a secret means an attacker needs the binary AND the key.

Modes
-----
No key  -> deterministic: every eligible function is lifted. This is what CI
           and `make demo` use, so the gate stays reproducible and bit-stable.
With key -> a keyed pseudo-random subset of the eligible pool is lifted.

The performance contract is unchanged in both modes: the key only chooses among
functions the tracer already approved. A key can never move a tight loop or
orchestration glue onto the slow path.
"""
from __future__ import annotations

import hashlib
import hmac
import json
import os

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)
KEY_FILE = os.path.join(REPO, ".lift_key")

# Fraction of the eligible pool a keyed deployment lifts. Chosen so a keyed
# deployment still moves meaningful work, while leaving real uncertainty about
# any individual function.
DEFAULT_FRACTION = 0.6

_KEY_ID_LABEL = b"craneliftX-lift-placement-v1"


def load_key() -> bytes | None:
    """Per-deployment lift key, or None for deterministic placement.

    Order: $LIFT_KEY (hex) then .lift_key at the repo root (hex, or raw bytes).
    """
    env = os.environ.get("LIFT_KEY", "").strip()
    if env:
        try:
            return bytes.fromhex(env)
        except ValueError:
            # Not hex: use the UTF-8 bytes so a passphrase works too.
            return env.encode()
    if os.path.isfile(KEY_FILE):
        with open(KEY_FILE, "rb") as fh:
            raw = fh.read().strip()
        if not raw:
            return None
        try:
            return bytes.fromhex(raw.decode())
        except (ValueError, UnicodeDecodeError):
            return raw
    return None


def key_fingerprint(key: bytes) -> str:
    """Non-secret identifier for a key, safe to print and to put in logs."""
    return hmac.new(key, _KEY_ID_LABEL, hashlib.sha256).hexdigest()[:16]


def _unit_interval(name: str, addr: int, key: bytes) -> float:
    """Keyed pseudo-random value in [0, 1) for one function."""
    msg = f"{name}:{addr:#x}".encode()
    digest = hmac.new(key, msg, hashlib.sha256).digest()
    return int.from_bytes(digest[:8], "big") / float(1 << 64)


def select(eligible: list[dict], key: bytes | None,
           fraction: float = DEFAULT_FRACTION) -> tuple[list[str], list[str]]:
    """Return (lift_these, hold_these) symbol names.

    With no key every eligible function is lifted (deterministic). With a key,
    each eligible function is included when its keyed value falls under
    `fraction`; at least one is always kept so the deployment still exercises
    the lifted path.
    """
    names = [f["name"] for f in eligible]
    if key is None:
        return names, []

    chosen = [
        f["name"] for f in eligible
        if _unit_interval(f["name"], int(f.get("addr", 0)), key) < fraction
    ]
    if not chosen and eligible:
        # Never lift nothing: fall back to the highest-scoring candidate.
        best = max(eligible, key=lambda f: float(f.get("lift_score", 0.0)))
        chosen = [best["name"]]
    held = [n for n in names if n not in chosen]
    return chosen, held


def plan(report: dict, key: bytes | None,
         fraction: float = DEFAULT_FRACTION) -> dict:
    """Build the lift plan for a tracer report."""
    eligible = [
        f for f in report.get("functions", [])
        if f.get("decision") == "LIFT" and f.get("liftable_now")
    ]
    chosen, held = select(eligible, key, fraction)
    entry = {
        "mode": "keyed" if key is not None else "deterministic",
        "fraction": fraction if key is not None else 1.0,
        "eligible": [f["name"] for f in eligible],
        "selected": chosen,
        "held_native": held,
    }
    if key is not None:
        # Fingerprint only -- never write the key itself anywhere.
        entry["key_fingerprint"] = key_fingerprint(key)
    return entry


if __name__ == "__main__":
    import sys

    report_path = os.path.join(HERE, "lift_map.json")
    if not os.path.isfile(report_path):
        raise SystemExit("run ingest_tracer.py first (no lift_map.json)")
    with open(report_path) as fh:
        rep = json.load(fh)
    k = load_key()
    print(json.dumps(plan(rep, k), indent=2))
