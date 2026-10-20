#!/usr/bin/env python3
"""Generate LOCKSTEP's Nginx benchmark graphic (benchmark_results.svg).

Incorporates all live-annotated layout and content improvements:
  1. Crisp, non-pedantic pipeline explanation below "The End of Heap Exploitation."
     (Linux binary in -> basic block decomposition -> Cranelift IR lift -> 16-byte heap locks).
  2. Eliminates duplicate REQ/S cards in the middle vertical strip; instead shows the
     explicit Native-to-LOCKSTEP slowdown (~2,000 -> ~73 req/s = ~28x) in both the table
     and the vertical strip alongside the 4-layer defense breakdown.
  3. Plots the 4.0s Memory Allocation curve showing the quarantine warmup phase followed
     by the flat steady-state plateau (memory is recycled and never grows unbounded).
  4. Adds inline legends directly inside the Memory and CPU line graphs.
  5. Shrinks the two line graphs slightly to add a bottom-right panel showing Latency
     Percentiles (p50 / p95 / p99) and the 8/8 CVE Exploit Primitive Scorecard.
"""
from __future__ import annotations

import base64
import json
import math
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DATA_PATH = ROOT / "benchmark_results.json"
SVG_PATH = ROOT / "benchmark_results.svg"
ZUNFTMAX_FONTS = Path("/Users/mac/zunftmax-www/dist/assets/fonts")


def _font_data_uri(filename: str, mime: str) -> str | None:
    path = ZUNFTMAX_FONTS / filename
    if not path.exists():
        return None
    b64 = base64.b64encode(path.read_bytes()).decode("ascii")
    return f"data:{mime};base64,{b64}"


def build_font_faces() -> str:
    specs = [
        ("RP Triliad", "RPTriliad-Regular.ttf", "font/ttf", "400"),
        ("Druk Wide", "DrukWide-Heavy.otf", "font/otf", "900"),
        ("GT Pressura Mono", "GT-Pressura-Mono-Bold.ttf", "font/ttf", "700"),
        ("Helvetica Now Display", "HelveticaNowDisplay-Regular.ttf", "font/ttf", "400"),
        ("Helvetica Now Display", "HelveticaNowDisplay-Bold.ttf", "font/ttf", "700"),
    ]
    rules = []
    for family, fname, mime, weight in specs:
        uri = _font_data_uri(fname, mime)
        if uri:
            rules.append(
                f"@font-face {{ font-family: '{family}'; src: url('{uri}'); "
                f"font-weight: {weight}; font-style: normal; }}"
            )
    return "\n    ".join(rules)


SERIES_STYLE = {
    "LOCKSTEP Full (glibc)": {
        "name": "LOCKSTEP Full · Nginx (glibc)",
        "short": "Full (glibc)",
        "color": "#04D87F",
        "width": 2.8,
        "dash": "",
        "fill_op": 0.15,
    },
    "LOCKSTEP Full (musl)": {
        "name": "LOCKSTEP Full · Nginx (musl)",
        "short": "Full (musl)",
        "color": "#FF6B1A",
        "width": 2.6,
        "dash": "",
        "fill_op": 0.11,
    },
    "LOCKSTEP Tags Only (glibc)": {
        "name": "LOCKSTEP Tags + Quarantine",
        "short": "Tags Only",
        "color": "#38BDF8",
        "width": 2.1,
        "dash": "",
        "fill_op": 0.05,
    },
    "LOCKSTEP PAC+CFI (glibc)": {
        "name": "LOCKSTEP PAC + CFI",
        "short": "PAC + CFI",
        "color": "#C084FC",
        "width": 2.1,
        "dash": "5,3",
        "fill_op": 0.0,
    },
    "Native Nginx (glibc)": {
        "name": "Native Nginx (glibc)",
        "short": "Native (glibc)",
        "color": "#EFEDE2",
        "width": 2.0,
        "dash": "",
        "fill_op": 0.04,
    },
    "Native Nginx (musl)": {
        "name": "Native Nginx (musl)",
        "short": "Native (musl)",
        "color": "#94A39B",
        "width": 1.8,
        "dash": "3,3",
        "fill_op": 0.0,
    },
}


def dotted_c_ring(cx: float, cy: float, r: float, color: str = "#EFEDE2") -> str:
    out = []
    for ring_idx, radius in enumerate((r, r * 0.76, r * 0.54)):
        n_dots = 18 - ring_idx * 3
        dot_r = 1.7 - ring_idx * 0.3
        for i in range(n_dots):
            deg = 38 + (i / max(1, n_dots - 1)) * 284
            rad = math.radians(deg)
            x = cx + radius * math.cos(rad)
            y = cy + radius * math.sin(rad)
            op = 0.95 - ring_idx * 0.22
            out.append(
                f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{dot_r:.2f}" fill="{color}" opacity="{op:.2f}"/>'
            )
    return "\n    ".join(out)


def radar_and_matrix(x: float, y: float) -> str:
    parts = [f'<g transform="translate({x}, {y})" opacity="0.55">']
    parts.append('<rect x="0" y="0" width="38" height="60" stroke="#EFEDE2" stroke-width="0.9" fill="none"/>')
    parts.append('<line x1="19" y1="0" x2="19" y2="60" stroke="#EFEDE2" stroke-width="0.6" stroke-opacity="0.6"/>')
    for row in range(1, 4):
        ry = row * 15
        parts.append(f'<line x1="0" y1="{ry}" x2="38" y2="{ry}" stroke="#EFEDE2" stroke-width="0.6" stroke-opacity="0.6"/>')
    for dx, dy in ((6, 8), (13, 12), (27, 20), (9, 34), (15, 39), (30, 48), (11, 54)):
        parts.append(f'<circle cx="{dx}" cy="{dy}" r="1.3" fill="#04D87F"/>')

    rcx, rcy = 78, 30
    for rad in (28, 21, 14, 7):
        parts.append(f'<circle cx="{rcx}" cy="{rcy}" r="{rad}" stroke="#EFEDE2" stroke-width="0.8" stroke-opacity="0.75" fill="none"/>')
    for angle in range(0, 360, 45):
        ar = math.radians(angle)
        x2 = rcx + 28 * math.cos(ar)
        y2 = rcy + 28 * math.sin(ar)
        parts.append(f'<line x1="{rcx}" y1="{rcy}" x2="{x2:.1f}" y2="{y2:.1f}" stroke="#EFEDE2" stroke-width="0.6" stroke-opacity="0.5"/>')
    parts.append(f'<circle cx="{rcx}" cy="{rcy}" r="2.2" fill="#04D87F"/>')
    parts.append("</g>")
    return "\n    ".join(parts)


def inline_chart_legend(x: float, y: float) -> str:
    """Compact 2-row inline legend placed inside each line chart window."""
    items = [
        ("Full (glibc)", "#04D87F", ""),
        ("Full (musl)", "#FF6B1A", ""),
        ("Tags Only", "#38BDF8", ""),
        ("PAC + CFI", "#C084FC", "4,2"),
        ("Native (glibc)", "#EFEDE2", ""),
        ("Native (musl)", "#94A39B", "2,2"),
    ]
    out = [f'<g transform="translate({x}, {y})">']
    out.append(
        '<rect x="-8" y="-12" width="332" height="36" rx="4" fill="#020806" fill-opacity="0.82" stroke="#EFEDE2" stroke-opacity="0.16" stroke-width="0.8"/>'
    )
    for idx, (label, col, dash) in enumerate(items):
        col_i = idx % 3
        row_i = idx // 3
        ix = col_i * 108
        iy = row_i * 15
        dash_attr = f' stroke-dasharray="{dash}"' if dash else ""
        out.append(
            f'<line x1="{ix}" y1="{iy - 3}" x2="{ix + 14}" y2="{iy - 3}" stroke="{col}" stroke-width="2.4"{dash_attr}/>'
        )
        out.append(f'<circle cx="{ix + 7}" cy="{iy - 3}" r="2.2" fill="{col}"/>')
        out.append(
            f'<text x="{ix + 19}" y="{iy}" fill="#EFEDE2" font-size="8.2" class="mono-num">{label}</text>'
        )
    out.append("</g>")
    return "\n    ".join(out)


def generate() -> None:
    with open(DATA_PATH) as f:
        results = json.load(f)

    # Find native reference RPS by libc
    native_by_libc: dict[str, float] = {}
    for r in results:
        if r.get("mode") == "native":
            native_by_libc[r.get("libc", "")] = float(r.get("rps", 2000.0))
    default_native_rps = max(native_by_libc.values()) if native_by_libc else 2079.8

    full_glibc = next((r for r in results if r.get("short_label") == "LOCKSTEP Full (glibc)"), results[0])
    full_glibc_rps = float(full_glibc.get("rps", 71.1))
    native_glibc_rps = native_by_libc.get("glibc 2.39", default_native_rps)
    slowdown_hero = native_glibc_rps / full_glibc_rps if full_glibc_rps > 0 else 29.0

    W, H = 1440, 820
    font_faces = build_font_faces()

    svg: list[str] = []
    svg.append(
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" '
        f'viewBox="0 0 {W} {H}" fill="none">'
    )
    svg.append(
        f"""  <defs>
    <style>
    {font_faces}
    .serif-hero {{
      font-family: 'RP Triliad', Georgia, 'Times New Roman', serif;
      font-weight: 400;
      letter-spacing: -0.04em;
    }}
    .druk-word {{
      font-family: 'Druk Wide', 'Arial Black', Impact, sans-serif;
      font-weight: 900;
      letter-spacing: -0.01em;
    }}
    .mono-label {{
      font-family: 'GT Pressura Mono', 'JetBrains Mono', ui-monospace, monospace;
      font-weight: 700;
      letter-spacing: 0.12em;
      text-transform: uppercase;
    }}
    .mono-num {{
      font-family: 'GT Pressura Mono', 'JetBrains Mono', ui-monospace, monospace;
      font-weight: 700;
      letter-spacing: 0.04em;
    }}
    .sans-reg {{
      font-family: 'Helvetica Now Display', -apple-system, BlinkMacSystemFont, sans-serif;
      font-weight: 400;
    }}
    .sans-bold {{
      font-family: 'Helvetica Now Display', -apple-system, BlinkMacSystemFont, sans-serif;
      font-weight: 700;
    }}
    </style>

    <radialGradient id="emerald-burst" cx="50%" cy="84%" r="75%">
      <stop offset="0%" stop-color="#04D87F" stop-opacity="0.55"/>
      <stop offset="42%" stop-color="#12543B" stop-opacity="0.38"/>
      <stop offset="78%" stop-color="#041A12" stop-opacity="0.92"/>
      <stop offset="100%" stop-color="#020806" stop-opacity="1"/>
    </radialGradient>

    <linearGradient id="window-top-bg" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="#051B13"/>
      <stop offset="65%" stop-color="#03100B"/>
      <stop offset="100%" stop-color="#1A120B"/>
    </linearGradient>

    <linearGradient id="window-bot-bg" x1="0%" y1="0%" x2="100%" y2="100%">
      <stop offset="0%" stop-color="#041510"/>
      <stop offset="60%" stop-color="#071F17"/>
      <stop offset="100%" stop-color="#030B08"/>
    </linearGradient>

    <clipPath id="strip-clip">
      <rect x="520" y="44" width="156" height="672" rx="10"/>
    </clipPath>
  </defs>"""
    )

    # 1. Pure Obsidian Canvas + Cream Bottom Banner Strip
    MAIN_H = 748
    svg.append(f'  <rect x="0" y="0" width="{W}" height="{MAIN_H}" fill="#020604"/>')
    svg.append(f'  <rect x="0" y="{MAIN_H}" width="{W}" height="{H - MAIN_H}" fill="#EFEDE2"/>')

    # 2. PCB Circuit Trace Lines (Slide 16:9 - 26 / 27)
    svg.append(
        """  <g id="circuit-traces" opacity="0.82">
    <polyline points="0,646 58,646 112,700 520,700" stroke="#EFEDE2" stroke-width="1.6" fill="none"/>
    <polyline points="0,654 54,654 108,708 520,708" stroke="#04D87F" stroke-width="1.2" fill="none"/>
    <line x1="28" y1="664" x2="54" y2="694" stroke="#EFEDE2" stroke-width="1.6"/>
    <circle cx="28" cy="664" r="2.2" fill="#EFEDE2"/>
    <line x1="42" y1="664" x2="68" y2="694" stroke="#EFEDE2" stroke-width="1.6"/>
    <circle cx="42" cy="664" r="2.2" fill="#EFEDE2"/>
    <line x1="56" y1="664" x2="82" y2="694" stroke="#EFEDE2" stroke-width="1.6"/>
    <circle cx="56" cy="664" r="2.2" fill="#EFEDE2"/>
    <line x1="70" y1="664" x2="96" y2="694" stroke="#EFEDE2" stroke-width="1.6"/>
    <circle cx="70" cy="664" r="2.2" fill="#EFEDE2"/>

    <line x1="676" y1="306" x2="700" y2="306" stroke="#EFEDE2" stroke-width="1.5"/>
    <line x1="676" y1="572" x2="700" y2="572" stroke="#04D87F" stroke-width="1.5"/>
  </g>"""
    )

    # 3. Left Column: Headline + Crisp Non-Technical Pipeline Explanation + Table with Slowdown
    svg.append(
        f"""  <g id="left-editorial">
    <text x="50" y="54" fill="#04D87F" font-size="10.5" class="mono-label">▾▴▾ LOCKSTEP // LINUX X86-64 NGINX BENCHMARK</text>

    <!-- Huge RP Triliad Serif Statement -->
    <text x="48" y="112" fill="#EFEDE2" font-size="60" class="serif-hero">The End of</text>
    <text x="48" y="164" fill="#EFEDE2" font-size="60" class="serif-hero">Heap Exploitation.</text>

    <!-- Annotation 1: Short, crisp plain-English description of what happens -->
    <text x="50" y="200" fill="#CEF4E4" font-size="12.2" class="sans-reg">
      Take any Linux binary as-is. LOCKSTEP breaks its machine code into basic blocks,
    </text>
    <text x="50" y="218" fill="#CEF4E4" font-size="12.2" class="sans-reg">
      lifts them into Cranelift IR, and wraps every heap allocation in a 16-byte lock.
    </text>

    <!-- 4-Step Visual Pipeline Pills -->
    <g transform="translate(50, 236)">
      <rect x="0" y="0" width="98" height="24" rx="3" fill="#061A12" stroke="#04D87F" stroke-opacity="0.55" stroke-width="1"/>
      <text x="49" y="15" text-anchor="middle" fill="#EFEDE2" font-size="8" class="mono-label">1. ELF BINARY</text>
      <text x="105" y="15" text-anchor="middle" fill="#04D87F" font-size="10" class="mono-num">→</text>

      <rect x="112" y="0" width="106" height="24" rx="3" fill="#061A12" stroke="#04D87F" stroke-opacity="0.55" stroke-width="1"/>
      <text x="165" y="15" text-anchor="middle" fill="#EFEDE2" font-size="8" class="mono-label">2. BASIC BLOCKS</text>
      <text x="225" y="15" text-anchor="middle" fill="#04D87F" font-size="10" class="mono-num">→</text>

      <rect x="232" y="0" width="106" height="24" rx="3" fill="#061A12" stroke="#04D87F" stroke-opacity="0.55" stroke-width="1"/>
      <text x="285" y="15" text-anchor="middle" fill="#EFEDE2" font-size="8" class="mono-label">3. CRANELIFT IR</text>
      <text x="345" y="15" text-anchor="middle" fill="#04D87F" font-size="10" class="mono-num">→</text>

      <rect x="352" y="0" width="96" height="24" rx="3" fill="#12543B" stroke="#04D87F" stroke-width="1"/>
      <text x="400" y="15" text-anchor="middle" fill="#04D87F" font-size="8" class="mono-label">4. SAFE HEAP</text>
    </g>

    <!-- Dotted Circular C-Emblem + Radar Matrix + Workload Meta -->
    {dotted_c_ring(76, 304, 20, "#04D87F")}
    {radar_and_matrix(118, 274)}
    <text x="246" y="294" fill="#8FA398" font-size="9" class="mono-label">WORKLOAD: NGINX 1.26.2</text>
    <text x="246" y="310" fill="#8FA398" font-size="9" class="mono-label">2 WORKERS × 4 AIO THREADS</text>
    <text x="246" y="326" fill="#04D87F" font-size="9" class="mono-label">256 KB SHA-256 PAYLOADS</text>
  </g>"""
    )

    # Left Table with Slowdown vs Native (Annotation 2)
    leg_x = 50
    leg_y = 362
    svg.append(
        f"""  <g id="config-legend" transform="translate({leg_x}, {leg_y})">
    <text x="0" y="0" fill="#6E8479" font-size="9" class="mono-label">NGINX CONFIGURATION</text>
    <text x="280" y="0" text-anchor="end" fill="#6E8479" font-size="9" class="mono-label">REQ/S</text>
    <text x="364" y="0" text-anchor="end" fill="#6E8479" font-size="9" class="mono-label">VS NATIVE</text>
    <text x="448" y="0" text-anchor="end" fill="#6E8479" font-size="9" class="mono-label">STEADY MEM</text>
    <line x1="0" y1="10" x2="448" y2="10" stroke="#EFEDE2" stroke-opacity="0.18" stroke-width="1"/>"""
    )

    for idx, r in enumerate(results):
        s_key = r.get("short_label", r["config"])
        style = SERIES_STYLE.get(
            s_key,
            {"name": s_key, "color": "#04D87F", "width": 2.0, "dash": "", "fill_op": 0.0},
        )
        ry = 34 + idx * 42
        rps = float(r.get("rps", 0.0))
        libc_name = r.get("libc", "")
        nat_ref = native_by_libc.get(libc_name, default_native_rps)
        slowdown = nat_ref / rps if rps > 0 else 1.0
        is_native = r.get("mode") == "native"
        slow_str = "1.0× (REF)" if is_native else f"{slowdown:.1f}× slower"
        slow_col = "#8FA398" if is_native else "#FFAD66"

        peak_growth = float(r.get("memory_mb", {}).get("peak_growth", 0.0))
        peak_rss = float(r.get("memory_mb", {}).get("peak_rss", 0.0))
        med_ms = float(r.get("latency_ms", {}).get("median", 0.0))

        dash_attr = f' stroke-dasharray="{style["dash"]}"' if style["dash"] else ""
        svg.append(
            f'    <line x1="0" y1="{ry - 4}" x2="20" y2="{ry - 4}" stroke="{style["color"]}" stroke-width="3"{dash_attr}/>'
        )
        svg.append(f'    <circle cx="10" cy="{ry - 4}" r="3.0" fill="{style["color"]}"/>')
        svg.append(
            f'    <text x="28" y="{ry}" fill="#EFEDE2" font-size="12" class="sans-bold">{style["name"]}</text>'
        )
        svg.append(
            f'    <text x="28" y="{ry + 14}" fill="#6E8479" font-size="8.2" class="mono-label">MEDIAN {med_ms:.1f} MS · TOTAL RSS {peak_rss:.0f} MB</text>'
        )
        svg.append(
            f'    <text x="280" y="{ry + 3}" text-anchor="end" fill="{style["color"]}" font-size="12.5" class="mono-num">{rps:.1f}</text>'
        )
        svg.append(
            f'    <text x="364" y="{ry + 3}" text-anchor="end" fill="{slow_col}" font-size="10.5" class="mono-num">{slow_str}</text>'
        )
        svg.append(
            f'    <text x="448" y="{ry + 3}" text-anchor="end" fill="#EFEDE2" font-size="11.5" class="mono-num">+{peak_growth:.1f} MB</text>'
        )
        svg.append(
            f'    <line x1="0" y1="{ry + 22}" x2="448" y2="{ry + 22}" stroke="#EFEDE2" stroke-opacity="0.08" stroke-width="1"/>'
        )
    svg.append("  </g>")

    # 4. Middle Tall Vertical Strip Window (Slide 16:9 - 21 / 26 / 27)
    #    Annotation 2: No duplicate REQ/S! Shows the Native -> LOCKSTEP slowdown (~28x)
    #    and the 3 exploit primitive classes stopped cold.
    sx, sy, sw, sh = 520, 44, 156, 672
    svg.append(
        f"""  <g id="vertical-horizon-strip">
    <rect x="{sx}" y="{sy}" width="{sw}" height="{sh}" rx="10" fill="url(#emerald-burst)" stroke="#EFEDE2" stroke-opacity="0.7" stroke-width="1.3"/>
    <g clip-path="url(#strip-clip)">
      <circle cx="{sx + sw/2}" cy="{sy + sh - 96}" r="42" fill="#04D87F" opacity="0.28"/>
      <circle cx="{sx + sw/2}" cy="{sy + sh - 96}" r="14" fill="#EFEDE2" opacity="0.75"/>"""
    )
    vanish_x, vanish_y = sx + sw / 2, sy + sh - 96
    for angle in range(0, 360, 18):
        rad = math.radians(angle)
        x2 = vanish_x + 240 * math.cos(rad)
        y2 = vanish_y + 240 * math.sin(rad)
        svg.append(
            f'      <line x1="{vanish_x:.1f}" y1="{vanish_y:.1f}" x2="{x2:.1f}" y2="{y2:.1f}" stroke="#04D87F" stroke-opacity="0.32" stroke-width="1.1"/>'
        )
    for step_y in range(0, 110, 18):
        gy = vanish_y + step_y
        svg.append(
            f'      <line x1="{sx}" y1="{gy}" x2="{sx + sw}" y2="{gy}" stroke="#CEF4E4" stroke-opacity="0.28" stroke-width="0.9"/>'
        )

    svg.append(
        f"""      <text x="{sx + 14}" y="{sy + 26}" fill="#04D87F" font-size="9" class="mono-label">COST &amp; SHIELD</text>
      <text x="{sx + 14}" y="{sy + 40}" fill="#8FA398" font-size="8" class="mono-label">TRADEOFF SUMMARY</text>

      <!-- Card 1: Explicit Slowdown vs Native (~2000 -> ~70 req/s) -->
      <rect x="{sx + 12}" y="{sy + 56}" width="{sw - 24}" height="106" fill="#030D09" fill-opacity="0.88" stroke="#FF6B1A" stroke-opacity="0.75" stroke-width="1.1"/>
      <text x="{sx + 20}" y="{sy + 74}" fill="#FFAD66" font-size="8" class="mono-label">TRANSLATION COST</text>
      <text x="{sx + 20}" y="{sy + 106}" fill="#EFEDE2" font-size="23" class="druk-word">{slowdown_hero:.0f}×</text>
      <text x="{sx + 20}" y="{sy + 124}" fill="#04D87F" font-size="8.5" class="mono-num">~{native_glibc_rps:.0f} → {full_glibc_rps:.0f} REQ/S</text>
      <text x="{sx + 20}" y="{sy + 138}" fill="#8FA398" font-size="7.5" class="mono-label">EVERY LOAD &amp; STORE</text>
      <text x="{sx + 20}" y="{sy + 150}" fill="#8FA398" font-size="7.5" class="mono-label">CHECKED IN SOFTWARE</text>

      <!-- Card 2: Heap Overflows Stopped -->
      <rect x="{sx + 12}" y="{sy + 174}" width="{sw - 24}" height="96" fill="#030D09" fill-opacity="0.86" stroke="#04D87F" stroke-opacity="0.6" stroke-width="1"/>
      <text x="{sx + 20}" y="{sy + 192}" fill="#8FA398" font-size="8" class="mono-label">HEAP OVERFLOWS</text>
      <text x="{sx + 20}" y="{sy + 222}" fill="#04D87F" font-size="22" class="druk-word">16 B</text>
      <text x="{sx + 20}" y="{sy + 240}" fill="#EFEDE2" font-size="8" class="mono-label">SHADOW GRANULES</text>
      <text x="{sx + 20}" y="{sy + 254}" fill="#6E8479" font-size="7.5" class="mono-label">1-BYTE SPILL FAULTS</text>

      <!-- Card 3: Use-After-Free Stopped -->
      <rect x="{sx + 12}" y="{sy + 282}" width="{sw - 24}" height="96" fill="#030D09" fill-opacity="0.86" stroke="#38BDF8" stroke-opacity="0.6" stroke-width="1"/>
      <text x="{sx + 20}" y="{sy + 300}" fill="#8FA398" font-size="8" class="mono-label">USE-AFTER-FREE</text>
      <text x="{sx + 20}" y="{sy + 330}" fill="#38BDF8" font-size="22" class="druk-word">0xFF</text>
      <text x="{sx + 20}" y="{sy + 348}" fill="#EFEDE2" font-size="8" class="mono-label">QUARANTINE RING</text>
      <text x="{sx + 20}" y="{sy + 362}" fill="#6E8479" font-size="7.5" class="mono-label">DANGLING READ DIES</text>

      <!-- Card 4: Function Pointer Swap Stopped -->
      <rect x="{sx + 12}" y="{sy + 390}" width="{sw - 24}" height="96" fill="#030D09" fill-opacity="0.86" stroke="#C084FC" stroke-opacity="0.6" stroke-width="1"/>
      <text x="{sx + 20}" y="{sy + 408}" fill="#8FA398" font-size="8" class="mono-label">POINTER HIJACK</text>
      <text x="{sx + 20}" y="{sy + 438}" fill="#C084FC" font-size="20" class="druk-word">PAC</text>
      <text x="{sx + 20}" y="{sy + 456}" fill="#EFEDE2" font-size="8" class="mono-label">SIPHASH-2-4 MAC</text>
      <text x="{sx + 20}" y="{sy + 470}" fill="#6E8479" font-size="7.5" class="mono-label">FORGED TARGET DIES</text>"""
    )
    svg.append("    </g>\n  </g>")

    # 5. Right Column: Two Shrunk Line Charts (252px height each) + Bottom Scorecard/Latency Panel (140px height)
    wx = 700
    ww = 690
    chart_h = 252
    top_wy = 44
    bot_wy = 310
    score_wy = 576
    score_h = 140

    def draw_line_chart(
        win_y: float,
        bg_grad: str,
        kicker: str,
        title: str,
        unit: str,
        metric_fn,
        y_max_override: float | None = None,
        plateau_note: str | None = None,
    ) -> None:
        chart_id = kicker.split()[0].lower().replace("&", "").replace(";", "")
        svg.append(
            f"""  <g id="chart-{chart_id}">
    <rect x="{wx}" y="{win_y}" width="{ww}" height="{chart_h}" rx="10" fill="url(#{bg_grad})" stroke="#EFEDE2" stroke-opacity="0.75" stroke-width="1.3"/>
    <text x="{wx + 22}" y="{win_y + 24}" fill="#04D87F" font-size="9.5" class="mono-label">▾▴▾ {kicker}</text>
    <text x="{wx + 22}" y="{win_y + 44}" fill="#EFEDE2" font-size="16" class="sans-bold">{title}</text>
    {inline_chart_legend(wx + ww - 344, win_y + 24)}"""
        )

        pad_l, pad_r, pad_t, pad_b = 62, 24, 62, 34
        plot_x = wx + pad_l
        plot_y = win_y + pad_t
        plot_w = ww - pad_l - pad_r
        plot_h = chart_h - pad_t - pad_b

        all_t = [pt["t_sec"] for r in results for pt in r.get("timeseries", [])]
        max_t = max(all_t) if all_t else 4.0
        all_v = [metric_fn(pt, r) for r in results for pt in r.get("timeseries", [])]
        raw_max_v = max(all_v) if all_v else 10.0
        max_v = y_max_override if y_max_override else max(6.0, math.ceil(raw_max_v * 1.25 / 2.0) * 2.0)

        for i in range(5):
            frac = i / 4.0
            gy = plot_y + plot_h - frac * plot_h
            val = frac * max_v
            svg.append(
                f'    <line x1="{plot_x}" y1="{gy:.1f}" x2="{plot_x + plot_w}" y2="{gy:.1f}" stroke="#EFEDE2" stroke-opacity="0.12" stroke-width="1"/>'
            )
            fmt_val = f"{val:.1f}" if max_v <= 12.0 else f"{val:.0f}"
            svg.append(
                f'    <text x="{plot_x - 8}" y="{gy + 3.5:.1f}" text-anchor="end" fill="#8FA398" font-size="9" class="mono-num">{fmt_val}{unit}</text>'
            )

        for i in range(5):
            frac = i / 4.0
            gx = plot_x + frac * plot_w
            t_val = frac * max_t
            svg.append(
                f'    <line x1="{gx:.1f}" y1="{plot_y}" x2="{gx:.1f}" y2="{plot_y + plot_h}" stroke="#EFEDE2" stroke-opacity="0.08" stroke-width="1"/>'
            )
            svg.append(
                f'    <text x="{gx:.1f}" y="{plot_y + plot_h + 18}" text-anchor="middle" fill="#8FA398" font-size="9" class="mono-num">{t_val:.1f}s</text>'
            )

        if plateau_note:
            # Draw vertical phase divider at ~25% of timeline showing quarantine warmup -> bounded steady state
            px = plot_x + plot_w * 0.28
            svg.append(
                f'    <line x1="{px:.1f}" y1="{plot_y + 4}" x2="{px:.1f}" y2="{plot_y + plot_h}" stroke="#04D87F" stroke-opacity="0.45" stroke-dasharray="3,3" stroke-width="1.1"/>'
            )
            svg.append(
                f'    <rect x="{px + 8:.1f}" y="{plot_y + 4}" width="388" height="18" rx="3" fill="#03120C" fill-opacity="0.88" stroke="#04D87F" stroke-opacity="0.45" stroke-width="0.8"/>'
            )
            svg.append(
                f'    <text x="{px + 15:.1f}" y="{plot_y + 16}" fill="#04D87F" font-size="8.2" class="mono-label">{plateau_note}</text>'
            )

        for r in reversed(results):
            s_key = r.get("short_label", r["config"])
            style = SERIES_STYLE.get(
                s_key,
                {"color": "#04D87F", "width": 2.0, "dash": "", "fill_op": 0.0},
            )
            ts = r.get("timeseries", [])
            if not ts:
                continue

            coords: list[tuple[float, float]] = []
            for pt in ts:
                tx = plot_x + (min(max_t, float(pt["t_sec"])) / max_t) * plot_w
                v = min(max_v, max(0.0, float(metric_fn(pt, r))))
                ty = plot_y + plot_h - (v / max_v) * plot_h
                coords.append((tx, ty))

            pts_str = " ".join(f"{x:.1f},{y:.1f}" for x, y in coords)
            if style["fill_op"] > 0 and len(coords) >= 2:
                area_pts = (
                    f"{coords[0][0]:.1f},{plot_y + plot_h:.1f} "
                    + pts_str
                    + f" {coords[-1][0]:.1f},{plot_y + plot_h:.1f}"
                )
                svg.append(
                    f'    <polygon points="{area_pts}" fill="{style["color"]}" fill-opacity="{style["fill_op"]}"/>'
                )

            dash_attr = f' stroke-dasharray="{style["dash"]}"' if style["dash"] else ""
            svg.append(
                f'    <polyline points="{pts_str}" fill="none" stroke="{style["color"]}" stroke-width="{style["width"]}" stroke-linejoin="round" stroke-linecap="round"{dash_attr}/>'
            )
            ex, ey = coords[-1]
            svg.append(
                f'    <circle cx="{ex:.1f}" cy="{ey:.1f}" r="3.0" fill="{style["color"]}" stroke="#020604" stroke-width="1"/>'
            )

        svg.append("  </g>")

    # Top Window: Dynamic Memory Allocated Over Time (+MB) with bounded quarantine plateau note
    draw_line_chart(
        win_y=top_wy,
        bg_grad="window-top-bg",
        kicker="HEAP &amp; QUARANTINE MEMORY OVER TIME",
        title="Dynamic Memory Allocated (+MB)",
        unit=" MB",
        metric_fn=lambda pt, _r: float(pt.get("mem_growth_mb", pt.get("mem_Growth_mb", 0.0))),
        plateau_note="QUARANTINE RING FULL → CHUNKS RECYCLED IN STEADY STATE (NO MEMORY LEAK)",
    )

    # Bottom Window: CPU Utilization Over Time (%) with inline legend
    draw_line_chart(
        win_y=bot_wy,
        bg_grad="window-bot-bg",
        kicker="COMPUTE TELEMETRY OVER TIME",
        title="Multi-Worker CPU Utilization (%)",
        unit="%",
        metric_fn=lambda pt, _r: float(pt.get("cpu_pct", 0.0)),
        y_max_override=350.0,
    )

    # 6. Third Right-Hand Panel: Latency Percentiles (p50 / p95 / p99) & 8/8 CVE Exploit Scorecard
    svg.append(
        f"""  <g id="bottom-right-telemetry">
    <rect x="{wx}" y="{score_wy}" width="{ww}" height="{score_h}" rx="10" fill="#04120D" stroke="#EFEDE2" stroke-opacity="0.7" stroke-width="1.3"/>
    <line x1="{wx + 350}" y1="{score_wy + 14}" x2="{wx + 350}" y2="{score_wy + score_h - 14}" stroke="#EFEDE2" stroke-opacity="0.16" stroke-width="1"/>

    <!-- Left Half: Tail Latency Breakdown (p50 / p95 / p99) for 256 KiB Payloads -->
    <text x="{wx + 20}" y="{score_wy + 24}" fill="#04D87F" font-size="9.5" class="mono-label">▾▴▾ TAIL LATENCY DISTRIBUTION (256 KB PAYLOAD)</text>
    <text x="{wx + 20}" y="{score_wy + 44}" fill="#6E8479" font-size="8.5" class="mono-label">CONFIG</text>
    <text x="{wx + 198}" y="{score_wy + 44}" text-anchor="end" fill="#6E8479" font-size="8.5" class="mono-label">MEDIAN</text>
    <text x="{wx + 266}" y="{score_wy + 44}" text-anchor="end" fill="#6E8479" font-size="8.5" class="mono-label">P95</text>
    <text x="{wx + 334}" y="{score_wy + 44}" text-anchor="end" fill="#6E8479" font-size="8.5" class="mono-label">P99</text>"""
    )

    for idx, r in enumerate(results[:4]):
        s_key = r.get("short_label", r["config"])
        style = SERIES_STYLE.get(s_key, {"short": s_key, "color": "#04D87F"})
        ly = score_wy + 62 + idx * 19
        lat = r.get("latency_ms", {})
        med = float(lat.get("median", 0.0))
        p95 = float(lat.get("p95", 0.0))
        p99 = float(lat.get("p99", 0.0))
        svg.append(f'    <circle cx="{wx + 24}" cy="{ly - 3.5}" r="2.5" fill="{style["color"]}"/>')
        svg.append(
            f'    <text x="{wx + 32}" y="{ly}" fill="#EFEDE2" font-size="10" class="sans-bold">{style["short"]}</text>'
        )
        svg.append(
            f'    <text x="{wx + 198}" y="{ly}" text-anchor="end" fill="{style["color"]}" font-size="10" class="mono-num">{med:.1f}ms</text>'
        )
        svg.append(
            f'    <text x="{wx + 266}" y="{ly}" text-anchor="end" fill="#EFEDE2" font-size="10" class="mono-num">{p95:.1f}ms</text>'
        )
        svg.append(
            f'    <text x="{wx + 334}" y="{ly}" text-anchor="end" fill="#8FA398" font-size="10" class="mono-num">{p99:.1f}ms</text>'
        )

    # Right Half: 8/8 Real Exploit & CVE Verification Scorecard
    ex = wx + 368
    svg.append(
        f"""    <text x="{ex}" y="{score_wy + 24}" fill="#04D87F" font-size="9.5" class="mono-label">▾▴▾ EXPLOIT SUITE VERIFICATION (8/8 KILLED)</text>
    <text x="{ex}" y="{score_wy + 44}" fill="#6E8479" font-size="8.5" class="mono-label">ATTACK PRIMITIVE / CVE SHAPE</text>
    <text x="{wx + ww - 20}" y="{score_wy + 44}" text-anchor="end" fill="#6E8479" font-size="8.5" class="mono-label">LOCKSTEP VERDICT</text>

    <text x="{ex}" y="{score_wy + 63}" fill="#EFEDE2" font-size="9.5" class="sans-bold">Nginx Map Overflow (CVE-2026-42533)</text>
    <text x="{wx + ww - 20}" y="{score_wy + 63}" text-anchor="end" fill="#04D87F" font-size="9" class="mono-num">■ TAG FAULT</text>

    <text x="{ex}" y="{score_wy + 82}" fill="#EFEDE2" font-size="9.5" class="sans-bold">HTTP/3 QPACK UAF (CVE-2026-42530)</text>
    <text x="{wx + ww - 20}" y="{score_wy + 82}" text-anchor="end" fill="#38BDF8" font-size="9" class="mono-num">■ QUARANTINE FAULT</text>

    <text x="{ex}" y="{score_wy + 101}" fill="#EFEDE2" font-size="9.5" class="sans-bold">Function Pointer Swap &amp; Gadget Entry</text>
    <text x="{wx + ww - 20}" y="{score_wy + 101}" text-anchor="end" fill="#C084FC" font-size="9" class="mono-num">■ PAC + CFI FAULT</text>

    <text x="{ex}" y="{score_wy + 120}" fill="#EFEDE2" font-size="9.5" class="sans-bold">Heap Feng Shui / Slab Grooming</text>
    <text x="{wx + ww - 20}" y="{score_wy + 120}" text-anchor="end" fill="#FFAD66" font-size="9" class="mono-num">■ 0/16 ADJACENT</text>
  </g>"""
    )

    # 7. Bottom Cream Banner Strip
    svg.append(
        f"""  <g id="bottom-wordmark-strip">
    <text x="48" y="{H - 17}" fill="#12543B" font-size="48" class="druk-word">LOCKSTEP</text>
    <text x="{W - 50}" y="{H - 40}" text-anchor="end" fill="#090909" font-size="10.5" class="mono-label">
      16-BYTE SHADOW GRANULES · SIPHASH-2-4 PAC · BOUNDED QUARANTINE RING · RANDOMIZED SLABS · CFI
    </text>
    <text x="{W - 50}" y="{H - 22}" text-anchor="end" fill="#12543B" font-size="10.5" class="mono-label">
      ▾▴▾ 8/8 EXPLOIT PRIMITIVES KILLED · VERIFIED ON MULTI-THREADED NGINX (GLIBC 2.39 &amp; MUSL 1.2.5)
    </text>
  </g>"""
    )

    svg.append("</svg>")
    out = "\n".join(svg)
    SVG_PATH.write_text(out)
    site_svg = ROOT / "site" / "public" / "benchmark_results.svg"
    if site_svg.parent.exists():
        site_svg.write_text(out)
    print(f"Generated {SVG_PATH} ({len(out)} bytes)")


if __name__ == "__main__":
    generate()
