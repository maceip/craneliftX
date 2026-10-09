#!/usr/bin/env python3
"""Render the ingest-tracer -> lift -> drop pipeline as a standalone HTML page.

Everything on the page comes from real artifacts produced by an actual run:
  liftmap/lift_map.json      ingest tracer analysis (AST/CFG metrics, decisions)
  liftmap/lift_plan.json     final placement (deterministic or keyed)
  liftmap/lift_results.json  what was actually lifted and validated

So the page is evidence, not illustration: the byte windows shown are the exact
bytes remill lifted, and the results are the values Pulley computed.

Usage: python3 liftmap/visualize.py [--out liftmap/pipeline_view.html]
"""
from __future__ import annotations

import argparse
import html
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(HERE)

LIFT_MAP = os.path.join(HERE, "lift_map.json")
LIFT_PLAN = os.path.join(HERE, "lift_plan.json")
LIFT_RESULTS = os.path.join(HERE, "lift_results.json")
DEFAULT_OUT = os.path.join(HERE, "pipeline_view.html")


def load(path: str) -> dict | None:
    if not os.path.isfile(path):
        return None
    with open(path) as fh:
        return json.load(fh)


def esc(value) -> str:
    return html.escape(str(value if value is not None else ""))


def actual_value(pulley: str, symbol: str) -> str:
    """Pull the computed result out of the Pulley runner's stdout."""
    if not pulley:
        return ""
    name = symbol.lstrip("_")
    for line in pulley.splitlines():
        if name in line and "=" in line:
            m = re.search(r"=\s*(-?\d+)", line)
            if m:
                return m.group(1)
    return ""


def out_values(pulley: str) -> list[str]:
    return re.findall(r"OUT\s+\w+\s+(-?\d+)", pulley or "")


def hexdump(hexstr: str, base: int, per_line: int = 16) -> str:
    """Format lifted bytes with offsets -- the exact window remill lifted."""
    try:
        raw = bytes.fromhex(hexstr)
    except ValueError:
        return ""
    out = []
    for off in range(0, len(raw), per_line):
        chunk = raw[off : off + per_line]
        hexpart = " ".join(f"{b:02x}" for b in chunk)
        out.append(f"{base + off:08x}  {hexpart}")
    return "\n".join(out)


STAGES = [
    ("Native .o", "x86-64 machine code"),
    ("Ingest tracer", "capstone AST + CFG"),
    ("Placement", "keyed / deterministic"),
    ("Anvill spec", "protobuf ABI + bytes"),
    ("remill + anvill", "lift to LLVM IR"),
    ("llc + wasm-ld", "IR to .wasm"),
    ("Cranelift / Pulley", "drop and interpret"),
]


def build_html(report, plan, results_doc) -> str:
    funcs = (report or {}).get("functions", [])
    results = (results_doc or {}).get("results", [])
    by_name = {r["name"]: r for r in results}
    mode = (plan or {}).get("mode", "unknown")
    fingerprint = (plan or {}).get("key_fingerprint")

    lifted_names = [r["name"] for r in results] or (plan or {}).get("selected", [])
    kept = [f["name"] for f in funcs if f.get("decision") != "LIFT"]
    kept += [n for n in (plan or {}).get("held_native", [])]

    # --- header ---
    parts = []
    parts.append(f"""<!DOCTYPE html>
<html lang="en"><head><meta charset="utf-8">
<title>craneliftX - lift and drop pipeline</title>
<style>
 :root {{ --bg:#f7f8fa; --card:#ffffff; --ink:#1b1f24; --muted:#5b6572;
          --line:#dfe3e8; --lift:#0a6c3f; --liftbg:#e6f4ec;
          --keep:#8a5a00; --keepbg:#fdf3e2; --accent:#205081; }}
 * {{ box-sizing:border-box; }}
 body {{ margin:0; padding:32px; background:var(--bg); color:var(--ink);
   font:15px/1.55 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,Helvetica,Arial,sans-serif; }}
 .wrap {{ max-width:1180px; margin:0 auto; }}
 h1 {{ font-size:26px; margin:0 0 4px; }}
 h2 {{ font-size:18px; margin:34px 0 12px; padding-bottom:6px;
   border-bottom:2px solid var(--line); }}
 .sub {{ color:var(--muted); margin-bottom:18px; }}
 .badges {{ display:flex; gap:10px; flex-wrap:wrap; margin:16px 0 6px; }}
 .badge {{ background:var(--card); border:1px solid var(--line);
   border-radius:6px; padding:6px 11px; font-size:13px; }}
 .badge b {{ color:var(--accent); }}
 /* pipeline */
 .flow {{ display:flex; align-items:stretch; gap:0; flex-wrap:wrap;
   background:var(--card); border:1px solid var(--line); border-radius:8px;
   padding:14px; }}
 .stage {{ flex:1 1 130px; min-width:120px; text-align:center; padding:8px 6px; }}
 .stage .n {{ font-weight:600; font-size:13px; }}
 .stage .d {{ color:var(--muted); font-size:11px; margin-top:2px; }}
 .arrow {{ align-self:center; color:var(--muted); padding:0 2px; font-size:15px; }}
 /* table */
 table {{ width:100%; border-collapse:collapse; background:var(--card);
   border:1px solid var(--line); border-radius:8px; overflow:hidden;
   font-size:13px; }}
 th,td {{ padding:8px 10px; text-align:left; border-bottom:1px solid var(--line); }}
 th {{ background:#eef1f5; font-size:12px; text-transform:uppercase;
   letter-spacing:.03em; color:var(--muted); }}
 tr:last-child td {{ border-bottom:none; }}
 .tag {{ display:inline-block; padding:2px 8px; border-radius:10px;
   font-size:11px; font-weight:600; }}
 .tag.lift {{ background:var(--liftbg); color:var(--lift); }}
 .tag.keep {{ background:var(--keepbg); color:var(--keep); }}
 .num {{ font-variant-numeric:tabular-nums; }}
 /* function cards */
 .card {{ background:var(--card); border:1px solid var(--line);
   border-radius:8px; padding:14px 16px; margin-bottom:12px; }}
 .card h3 {{ margin:0 0 6px; font-size:15px; font-family:ui-monospace,SFMono-Regular,Menlo,monospace; }}
 .meta {{ color:var(--muted); font-size:12px; margin-bottom:8px; }}
 .meta code {{ color:var(--ink); }}
 pre {{ background:#f2f4f7; border:1px solid var(--line); border-radius:6px;
   padding:10px; overflow-x:auto; font:12px/1.5 ui-monospace,SFMono-Regular,Menlo,monospace;
   margin:8px 0; }}
 .ok {{ color:var(--lift); font-weight:600; }}
 footer {{ margin-top:30px; color:var(--muted); font-size:12px;
   border-top:1px solid var(--line); padding-top:12px; }}
 .note {{ background:#eef4fb; border-left:3px solid var(--accent);
   padding:9px 12px; font-size:13px; margin:12px 0; }}
</style></head><body><div class="wrap">""")

    parts.append(
        "<h1>Native &rarr; Pulley lift and drop</h1>"
        '<div class="sub">Static analysis decides <em>where</em> to lift; remill/anvill '
        "lift it; Cranelift/Pulley runs it. Every value below comes from a real run.</div>"
    )

    parts.append('<div class="badges">')
    parts.append(f'<div class="badge">functions analyzed <b>{len(funcs)}</b></div>')
    parts.append(f'<div class="badge">lifted &amp; validated <b>{len(lifted_names)}</b></div>')
    parts.append(f'<div class="badge">kept native <b>{len(set(kept))}</b></div>')
    parts.append(f'<div class="badge">placement <b>{esc(mode)}</b></div>')
    if fingerprint:
        parts.append(f'<div class="badge">key fingerprint <b>{esc(fingerprint)}</b></div>')
    parts.append("</div>")

    if not funcs:
        parts.append(
            '<div class="note">No <code>lift_map.json</code> found. Run '
            "<code>make demo</code> first &mdash; this page renders real artifacts.</div>"
        )

    # --- pipeline flow ---
    parts.append("<h2>Pipeline</h2><div class='flow'>")
    for i, (name, desc) in enumerate(STAGES):
        if i:
            parts.append('<div class="arrow">&rarr;</div>')
        parts.append(f'<div class="stage"><div class="n">{esc(name)}</div>'
                     f'<div class="d">{esc(desc)}</div></div>')
    parts.append("</div>")
    parts.append(
        '<div class="note">The tracer hands the lift op the exact byte window '
        "(<code>addr</code>/<code>size_bytes</code>) and the recovered ABI "
        "<code>signature</code> &mdash; the lift does not re-derive them.</div>"
    )

    # --- decisions table ---
    parts.append("<h2>Static analysis: every function</h2>")
    parts.append("<table><thead><tr><th>Function</th><th>Decision</th>"
                 "<th>Addr</th><th>Size</th><th>Signature</th>"
                 "<th>Loop</th><th>Calls</th><th>Net</th><th>Why</th></tr></thead><tbody>")
    for f in sorted(funcs, key=lambda x: (x.get("decision") != "LIFT", -x.get("lift_score", 0))):
        is_lift = f.get("decision") == "LIFT"
        cls = "lift" if is_lift else "keep"
        parts.append(
            f"<tr><td><code>{esc(f.get('name'))}</code></td>"
            f'<td><span class="tag {cls}">{esc(f.get("decision"))}</span></td>'
            f'<td class="num">0x{int(f.get("addr", 0)):x}</td>'
            f'<td class="num">{esc(f.get("size_bytes"))}</td>'
            f"<td><code>{esc(f.get('signature'))}</code></td>"
            f'<td class="num">{esc(f.get("loop_density"))}</td>'
            f'<td class="num">{esc(f.get("call_fraction"))}</td>'
            f'<td class="num">{esc(f.get("network_score"))}</td>'
            f"<td>{esc(f.get('reason'))}</td></tr>"
        )
    parts.append("</tbody></table>")

    # --- lifted detail ---
    parts.append("<h2>What actually got lifted</h2>")
    if not results:
        parts.append(
            '<div class="note">No <code>lift_results.json</code> yet &mdash; run '
            "<code>make demo</code> to populate byte windows and validation.</div>"
        )
    for r in sorted(results, key=lambda x: x["name"]):
        addr = int(r.get("addr", 0))
        size = int(r.get("size_bytes", 0))
        got = actual_value(r.get("pulley", ""), r.get("name", ""))
        outs = out_values(r.get("pulley", ""))
        dump = hexdump(r.get("hexbytes", ""), addr)
        parts.append('<div class="card">')
        parts.append(f"<h3>{esc(r.get('name'))}</h3>")
        parts.append(
            f'<div class="meta">window <code>0x{addr:x}&ndash;0x{addr + size:x}</code> '
            f"({size} bytes) &middot; signature <code>{esc(r.get('signature'))}</code> "
            f"&middot; loop_density <code>{esc(r.get('loop_density'))}</code></div>"
        )
        if dump:
            parts.append(
                "<div class='meta'>Machine code handed to remill "
                "(tracer-authoritative window):</div>"
            )
            parts.append(f"<pre>{esc(dump)}</pre>")
        exp = r.get("expected")
        exp_outs = r.get("expected_outs") or []
        verdict = "ok" if str(got) == str(exp) else ""
        line = f"Pulley result <span class='{verdict}'>{esc(got)}</span> " \
               f"(expected {esc(exp)})"
        if exp_outs:
            line += " &middot; out params " + esc(", ".join(str(o) for o in exp_outs))
            if outs:
                line += " (Pulley: " + esc(", ".join(outs)) + ")"
        parts.append(f"<div>{line}</div>")
        qemu = (r.get("qemu") or "").strip()
        parts.append(
            f'<div class="meta">qemu-riscv64: {esc(qemu) if qemu else "skipped (not installed)"}</div>'
        )
        parts.append("</div>")

    # --- kept native ---
    parts.append("<h2>Deliberately kept native</h2><table><thead><tr>"
                 "<th>Function</th><th>Reason</th></tr></thead><tbody>")
    for f in funcs:
        if f.get("decision") == "LIFT":
            continue
        parts.append(f"<tr><td><code>{esc(f.get('name'))}</code></td>"
                     f"<td>{esc(f.get('reason'))}</td></tr>")
    for n in (plan or {}).get("held_native", []):
        parts.append(f"<tr><td><code>{esc(n)}</code></td>"
                     "<td>keyed placement kept this one native this deployment</td></tr>")
    parts.append("</tbody></table>")

    parts.append(
        "<footer>Reproduce: <code>make demo</code> then "
        "<code>python3 liftmap/visualize.py</code>. "
        "Artifacts: lift_map.json (tracer), lift_plan.json (placement), "
        "lift_results.json (validated lifts).</footer>"
    )
    parts.append("</div></body></html>")
    return "".join(parts)


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Visualize the lift/drop pipeline")
    ap.add_argument("--out", default=DEFAULT_OUT)
    ns = ap.parse_args(argv)

    report = load(LIFT_MAP)
    plan = load(LIFT_PLAN)
    results_doc = load(LIFT_RESULTS)
    if report is None and results_doc is None:
        sys.stderr.write("no artifacts found; run 'make demo' first\n")
        return 1

    html_text = build_html(report, plan, results_doc)
    with open(ns.out, "w") as fh:
        fh.write(html_text)
    print(f"wrote {ns.out} ({len(html_text)} bytes)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
