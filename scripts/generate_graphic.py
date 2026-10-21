#!/usr/bin/env python3
"""Render recorded Nginx results as a plain-language, Office 97-style Lockstep SVG.

This renderer only reads benchmark_results.json. It does not execute benchmarks
or infer security results from performance measurements.
"""
from __future__ import annotations

import argparse
import json
import math
from html import escape
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
STYLES = {
    "LOCKSTEP Full (glibc)": ("All protections · glibc", "#000080", ""),
    "LOCKSTEP Full (musl)": ("All protections · musl", "#008000", ""),
    "LOCKSTEP Tags Only (glibc)": ("Memory checks only · glibc", "#800000", ""),
    "LOCKSTEP PAC+CFI (glibc)": ("Call checks only · glibc", "#800080", ""),
    "Native Nginx (glibc)": ("Unprotected · glibc", "#008080", "6 3"),
    "Native Nginx (musl)": ("Unprotected · musl", "#444444", "3 3"),
}


def generate(data_path: Path, svg_path: Path, site_path: Path | None) -> None:
    results = json.loads(data_path.read_text())
    if not isinstance(results, list) or not results:
        raise ValueError("Expected a nonempty list of recorded benchmark results")
    for row in results:
        if row["short_label"] not in STYLES:
            raise ValueError(f"Add a plain-language label for {row['short_label']!r}")
        for value in (row["rps"], row["latency_ms"]["median"]):
            if not math.isfinite(value) or value <= 0:
                raise ValueError("Throughput and median response time must be positive")
        if not row.get("timeseries"):
            raise ValueError("Memory and processor charts require recorded samples")
    native = {r["libc"]: r["rps"] for r in results if r["mode"] == "native"}
    if any(r["libc"] not in native for r in results):
        raise ValueError("Each library requires its own unprotected reference run")

    width, height = 1280, 920
    svg = [
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" '
        f'viewBox="0 0 {width} {height}" role="img" aria-labelledby="title description">',
        '<title id="title">Lockstep — Linux service benchmark</title>',
        '<desc id="description">A recorded Nginx benchmark comparing all protections, '
        'selected protections, and unprotected execution. The table shows requests '
        'per second, median response time, and the throughput cost relative to '
        'unprotected execution with the same library. Charts show memory increase '
        'and processor use during the recorded window. This is not a new benchmark '
        'or a security verification.</desc>',
        '<style>text { font-family: Arial, Helvetica, sans-serif; fill: #101010; } '
        '.number { font-family: "Courier New", monospace; font-variant-numeric: tabular-nums; }'
        '</style>',
    ]

    def rect(x, y, w, h, fill, stroke=None):
        edge = f' stroke="{stroke}"' if stroke else ""
        svg.append(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="{fill}"{edge}/>')

    def line(x1, y1, x2, y2, color="#808080", weight=1, dash=""):
        pattern = f' stroke-dasharray="{dash}"' if dash else ""
        svg.append(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" '
                   f'stroke="{color}" stroke-width="{weight}"{pattern}/>')

    def text(x, y, label, size=16, bold=False, color="#101010", anchor="start", number=False):
        cls = ' class="number"' if number else ""
        weight = ' font-weight="700"' if bold else ""
        svg.append(f'<text x="{x}" y="{y}" font-size="{size}" text-anchor="{anchor}"'
                   f' style="fill:{color}"{weight}{cls}>{escape(str(label))}</text>')

    def bevel(x, y, w, h, inset=False):
        light, dark = ("#808080", "#ffffff") if inset else ("#ffffff", "#808080")
        line(x, y, x+w, y, light)
        line(x, y, x, y+h, light)
        line(x, y+h, x+w, y+h, dark)
        line(x+w, y, x+w, y+h, dark)

    # Familiar Office-era window frame, with a quiet white document underneath.
    rect(0, 0, width, height, "#c0c0c0")
    bevel(2, 2, width-5, height-5)
    rect(7, 7, width-14, 34, "#000080")
    rect(16, 15, 18, 18, "#ffffff")
    for offset in (6, 12):
        line(16+offset, 15, 16+offset, 33, "#008000")
        line(16, 15+offset, 34, 15+offset, "#008000")
    text(44, 31, "Lockstep — Linux service benchmark", 19, True, "#ffffff")
    for x, symbol in ((1181, "_"), (1209, "□"), (1237, "×")):
        rect(x, 13, 24, 23, "#c0c0c0")
        bevel(x, 13, 24, 23)
        text(x+12, 30, symbol, 18, True, anchor="middle")
    text(20, 64, "File     Edit     View     Insert     Format     Help", 16)
    line(8, 73, 1271, 73, "#808080")
    line(8, 74, 1271, 74, "#ffffff")
    rect(16, 84, 1248, 785, "#ffffff")
    bevel(16, 84, 1248, 785, inset=True)

    text(42, 129, "A harness for Linux services.", 32, True)
    text(42, 162, "Blocks heap attacks using memory checks and protected function calls.", 22)
    text(42, 190, "Recorded Nginx performance. Protection is layered; coverage depends on the attack and configuration.", 15, color="#505050")

    # Spreadsheet-style comparison, with no marketing scorecards or invented evidence.
    x, y, table_w, header_h, row_h = 42, 215, 1196, 36, 37
    columns = [x, 454, 720, 996, x+table_w]
    table_h = header_h + len(results)*row_h
    rect(x, y, table_w, table_h, "#ffffff", "#808080")
    rect(x, y, table_w, header_h, "#d4d0c8")
    headers = [(x+16, "Configuration", "start"),
               (columns[2]-22, "Requests / second ↑", "end"),
               (columns[3]-22, "Response time ↓", "end"),
               (columns[4]-22, "Throughput cost ↓", "end")]
    for tx, label, anchor in headers:
        text(tx, y+24, label, 16, True, anchor=anchor)
    for i, row in enumerate(results):
        name, color, dash = STYLES[row["short_label"]]
        ry = y+header_h+i*row_h
        if i % 2 == 0:
            rect(x+1, ry, table_w-2, row_h, "#f3f5f7")
        line(x, ry, x+table_w, ry, "#c4c4c4")
        line(x+15, ry+19, x+42, ry+19, color, 3, dash)
        text(x+54, ry+25, name, 17)
        text(columns[2]-22, ry+25, f'{row["rps"]:,.1f}', 18, anchor="end", number=True)
        text(columns[3]-22, ry+25, f'{row["latency_ms"]["median"]:,.1f} ms', 18, anchor="end", number=True)
        cost = "Reference" if row["mode"] == "native" else f'{native[row["libc"]] / row["rps"]:.1f}× slower'
        text(columns[4]-22, ry+25, cost, 17, anchor="end", number=True)
    for cx in columns[1:-1]:
        line(cx, y, cx, y+table_h, "#b2b2b2")
    text(x, y+table_h+23, "Response time is the median. Each protection setting is compared with its own unprotected library.", 14, color="#505050")

    def chart(cx, cy, cw, ch, title, unit, metric):
        rect(cx, cy, cw, ch, "#ffffff", "#808080")
        rect(cx+1, cy+1, cw-2, 33, "#e8e8e8")
        text(cx+14, cy+23, title, 18, True)
        px, py = cx+65, cy+53
        pw, ph = cw-85, ch-89
        samples = [p for r in results for p in r["timeseries"]]
        max_t = max(float(p["t_sec"]) for p in samples)
        max_value = max(metric(p) for p in samples)
        if max_t <= 0 or not math.isfinite(max_value) or max_value < 0:
            raise ValueError("Invalid chart samples")
        magnitude = 10 ** math.floor(math.log10(max_value or 1))
        limit = max(magnitude, math.ceil(max_value / magnitude) * magnitude)
        for step in range(5):
            fraction = step/4
            gy = py+ph*(1-fraction)
            line(px, gy, px+pw, gy, "#d3d3d3")
            value = limit*fraction
            text(px-10, gy+5, f"{value:g}{unit}", 13, anchor="end")
            gx = px+pw*fraction
            text(gx, py+ph+23, f"{max_t*fraction:.1f}s", 13, anchor="middle")
        line(px, py, px, py+ph, "#606060")
        line(px, py+ph, px+pw, py+ph, "#606060")
        for row in reversed(results):
            _, color, dash = STYLES[row["short_label"]]
            points = " ".join(f'{px+float(p["t_sec"])/max_t*pw:.2f},{py+ph-metric(p)/limit*ph:.2f}' for p in row["timeseries"])
            pattern = f' stroke-dasharray="{dash}"' if dash else ""
            svg.append(f'<polyline points="{points}" fill="none" stroke="{color}" stroke-width="2.2"{pattern}/>')

    chart(42, 523, 585, 259, "Memory increase during the run", " MB",
          lambda p: float(p.get("mem_growth_mb", p.get("mem_Growth_mb", 0))))
    chart(653, 523, 585, 259, "Processor use across all workers", "%",
          lambda p: float(p["cpu_pct"]))
    for i, row in enumerate(results):
        name, color, dash = STYLES[row["short_label"]]
        lx, ly = 48+(i % 3)*402, 810+(i//3)*27
        line(lx, ly-5, lx+25, ly-5, color, 3, dash)
        text(lx+34, ly, name, 15)

    # State the evidence boundary without turning the chart into a wall of text.
    rect(16, 880, 1248, 25, "#c0c0c0")
    bevel(16, 880, 1248, 25, inset=True)
    durations = [r["duration_seconds"] for r in results]
    duration = f"{min(durations):.1f}–{max(durations):.1f}s" if max(durations)-min(durations) >= .1 else f"~{max(durations):.0f}s"
    text(24, 898, f"Source: {data_path.name}  |  Recorded window: {duration} per setting  |  Processor use can exceed 100% across cores.", 13)
    svg.append("</svg>")
    output = "\n".join(svg) + "\n"
    svg_path.write_text(output)
    if site_path is not None:
        site_path.write_text(output)
    print(f"Generated {svg_path} ({len(output.encode())} bytes) from {data_path}")
    if site_path is not None:
        print(f"Copied to {site_path}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", type=Path, default=ROOT / "benchmark_results.json")
    parser.add_argument("--svg", type=Path, default=ROOT / "benchmark_results.svg")
    args = parser.parse_args()
    site_path = ROOT / "site/public/benchmark_results.svg"
    generate(args.json, args.svg, site_path if site_path.parent.exists() else None)


if __name__ == "__main__":
    main()
