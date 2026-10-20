#!/usr/bin/env python3
"""Generate LOCKSTEP's Nginx benchmark graphic (benchmark_results.svg).

Visual composition directly follows the reference slide deck:
  - Slide 16:9 - 26 / 27: Deep black ground with generous left negative space,
    45-degree PCB circuit trace lines with 4 parallel diagonal hash ticks,
    a tall vertical perspective-ray strip window, and two stacked framed windows on the right.
  - Slide 16:9 - 31 / 222: Dotted circular C-ring emblem, concentric radar target +
    dot-matrix grid, and large RP Triliad serif headline ("The End of Heap Exploitation.").
  - Slide 16:9 - 156 / 170: Bold Druk Wide LOCKSTEP wordmark lockup.
  - Plots real Nginx time-series line graphs of Memory (MB) and CPU (%) over time.
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
        "color": "#04D87F",
        "width": 3.0,
        "dash": "",
        "fill_op": 0.16,
    },
    "LOCKSTEP Full (musl)": {
        "name": "LOCKSTEP Full · Nginx (musl)",
        "color": "#FF6B1A",
        "width": 2.8,
        "dash": "",
        "fill_op": 0.12,
    },
    "LOCKSTEP Tags Only (glibc)": {
        "name": "LOCKSTEP Tags + Quarantine",
        "color": "#38BDF8",
        "width": 2.2,
        "dash": "",
        "fill_op": 0.06,
    },
    "LOCKSTEP PAC+CFI (glibc)": {
        "name": "LOCKSTEP PAC + CFI",
        "color": "#C084FC",
        "width": 2.2,
        "dash": "5,3",
        "fill_op": 0.0,
    },
    "Native Nginx (glibc)": {
        "name": "Native Nginx (glibc)",
        "color": "#EFEDE2",
        "width": 2.2,
        "dash": "",
        "fill_op": 0.05,
    },
    "Native Nginx (musl)": {
        "name": "Native Nginx (musl)",
        "color": "#94A39B",
        "width": 1.8,
        "dash": "3,3",
        "fill_op": 0.0,
    },
}


def dotted_c_ring(cx: float, cy: float, r: float, color: str = "#EFEDE2") -> str:
    """Render the signature dotted circular C-emblem from Slide 16:9 - 15/16/25/222."""
    out = []
    for ring_idx, radius in enumerate((r, r * 0.76, r * 0.54)):
        n_dots = 18 - ring_idx * 3
        dot_r = 1.9 - ring_idx * 0.35
        for i in range(n_dots):
            deg = 38 + (i / max(1, n_dots - 1)) * 284  # open gap on the right like a 'C'
            rad = math.radians(deg)
            x = cx + radius * math.cos(rad)
            y = cy + radius * math.sin(rad)
            op = 0.95 - ring_idx * 0.22
            out.append(
                f'<circle cx="{x:.1f}" cy="{y:.1f}" r="{dot_r:.2f}" fill="{color}" opacity="{op:.2f}"/>'
            )
    return "\n    ".join(out)


def radar_and_matrix(x: float, y: float) -> str:
    """Render the dot-matrix rectangle + concentric radar target from Slide 16:9 - 31."""
    parts = [f'<g transform="translate({x}, {y})" opacity="0.55">']
    # Left 2x4 grid box with plotted dots (Slide 31)
    parts.append('<rect x="0" y="0" width="48" height="84" stroke="#EFEDE2" stroke-width="0.9" fill="none"/>')
    parts.append('<line x1="24" y1="0" x2="24" y2="84" stroke="#EFEDE2" stroke-width="0.6" stroke-opacity="0.6"/>')
    for row in range(1, 4):
        ry = row * 21
        parts.append(f'<line x1="0" y1="{ry}" x2="48" y2="{ry}" stroke="#EFEDE2" stroke-width="0.6" stroke-opacity="0.6"/>')
    dots = [(8, 12), (16, 18), (34, 28), (11, 46), (19, 52), (38, 66), (14, 74)]
    for dx, dy in dots:
        parts.append(f'<circle cx="{dx}" cy="{dy}" r="1.4" fill="#04D87F"/>')

    # Right concentric radar target (Slide 31)
    rcx, rcy = 96, 42
    for rad in (36, 27, 18, 9):
        parts.append(f'<circle cx="{rcx}" cy="{rcy}" r="{rad}" stroke="#EFEDE2" stroke-width="0.8" stroke-opacity="0.75" fill="none"/>')
    for angle in range(0, 360, 45):
        ar = math.radians(angle)
        x2 = rcx + 36 * math.cos(ar)
        y2 = rcy + 36 * math.sin(ar)
        parts.append(f'<line x1="{rcx}" y1="{rcy}" x2="{x2:.1f}" y2="{y2:.1f}" stroke="#EFEDE2" stroke-width="0.6" stroke-opacity="0.5"/>')
    parts.append(f'<circle cx="{rcx}" cy="{rcy}" r="2.5" fill="#04D87F"/>')
    parts.append("</g>")
    return "\n    ".join(parts)


def generate() -> None:
    with open(DATA_PATH) as f:
        results = json.load(f)

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
      letter-spacing: 0.13em;
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

    <!-- Atmospheric gradients from Slide 16:9 - 18 / 26 / 27 -->
    <radialGradient id="emerald-burst" cx="50%" cy="82%" r="75%">
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
      <rect x="520" y="54" width="156" height="656" rx="10"/>
    </clipPath>
  </defs>"""
    )

    # 1. Pure Obsidian Canvas + Cream Bottom Banner Strip (Slide 16:9 - 26 & 170)
    MAIN_H = 748
    svg.append(f'  <rect x="0" y="0" width="{W}" height="{MAIN_H}" fill="#020604"/>')
    svg.append(f'  <rect x="0" y="{MAIN_H}" width="{W}" height="{H - MAIN_H}" fill="#EFEDE2"/>')

    # 2. PCB Circuit Trace Lines from Slide 16:9 - 26 / 27
    # Bottom-left double trace with 4 diagonal hash ticks connecting into the vertical strip window
    svg.append(
        """  <g id="circuit-traces" opacity="0.82">
    <polyline points="0,636 58,636 112,690 520,690" stroke="#EFEDE2" stroke-width="1.6" fill="none"/>
    <polyline points="0,644 54,644 108,698 520,698" stroke="#04D87F" stroke-width="1.2" fill="none"/>
    <!-- 4 parallel diagonal hash ticks (exact detail from Slide 26/27) -->
    <line x1="28" y1="654" x2="54" y2="684" stroke="#EFEDE2" stroke-width="1.6"/>
    <circle cx="28" cy="654" r="2.2" fill="#EFEDE2"/>
    <line x1="42" y1="654" x2="68" y2="684" stroke="#EFEDE2" stroke-width="1.6"/>
    <circle cx="42" cy="654" r="2.2" fill="#EFEDE2"/>
    <line x1="56" y1="654" x2="82" y2="684" stroke="#EFEDE2" stroke-width="1.6"/>
    <circle cx="56" cy="654" r="2.2" fill="#EFEDE2"/>
    <line x1="70" y1="654" x2="96" y2="684" stroke="#EFEDE2" stroke-width="1.6"/>
    <circle cx="70" cy="654" r="2.2" fill="#EFEDE2"/>

    <!-- Bridge trace between vertical strip and right stacked windows -->
    <line x1="676" y1="368" x2="700" y2="368" stroke="#EFEDE2" stroke-width="1.5"/>
    <line x1="676" y1="396" x2="700" y2="396" stroke="#04D87F" stroke-width="1.5"/>
  </g>"""
    )

    # 3. Left Column: Generous Negative Space + LOCKSTEP Wordmark + Serif Headline + Legend
    svg.append(
        f"""  <g id="left-editorial">
    <text x="54" y="64" fill="#04D87F" font-size="11" class="mono-label">▾▴▾ LOCKSTEP // LINUX X86-64 NGINX BENCHMARK</text>

    <!-- Huge RP Triliad Serif Statement (Slide 15 / 20 / 222) -->
    <text x="52" y="138" fill="#EFEDE2" font-size="68" class="serif-hero">The End of</text>
    <text x="52" y="200" fill="#EFEDE2" font-size="68" class="serif-hero">Heap Exploitation.</text>

    <text x="56" y="232" fill="#8FA398" font-size="11" class="mono-label">
      REAL NGINX (GLIBC 2.39 &amp; MUSL 1.2.5) · 2 WORKERS × 4 THREADS
    </text>

    <!-- Dotted Circular C-Emblem (Slide 16/25/222) + Radar Matrix (Slide 31) -->
    {dotted_c_ring(86, 286, 24, "#04D87F")}
    {radar_and_matrix(140, 246)}
  </g>"""
    )

    # Clean Legend + Summary Table in Left Column (y = 356..620)
    leg_x = 54
    leg_y = 362
    svg.append(
        f"""  <g id="config-legend" transform="translate({leg_x}, {leg_y})">
    <text x="0" y="0" fill="#6E8479" font-size="9.5" class="mono-label">NGINX CONFIGURATION</text>
    <text x="318" y="0" text-anchor="end" fill="#6E8479" font-size="9.5" class="mono-label">REQ/S</text>
    <text x="426" y="0" text-anchor="end" fill="#6E8479" font-size="9.5" class="mono-label">PEAK MEM</text>
    <line x1="0" y1="10" x2="426" y2="10" stroke="#EFEDE2" stroke-opacity="0.18" stroke-width="1"/>"""
    )

    for idx, r in enumerate(results):
        s_key = r.get("short_label", r["config"])
        style = SERIES_STYLE.get(
            s_key,
            {"name": s_key, "color": "#04D87F", "width": 2.0, "dash": "", "fill_op": 0.0},
        )
        ry = 34 + idx * 40
        rps = float(r.get("rps", 0.0))
        peak_growth = float(r.get("memory_mb", {}).get("peak_growth", 0.0))
        peak_rss = float(r.get("memory_mb", {}).get("peak_rss", 0.0))
        med_ms = float(r.get("latency_ms", {}).get("median", 0.0))

        # Swatch line + node dot
        dash_attr = f' stroke-dasharray="{style["dash"]}"' if style["dash"] else ""
        svg.append(
            f'    <line x1="0" y1="{ry - 4}" x2="22" y2="{ry - 4}" stroke="{style["color"]}" stroke-width="3"{dash_attr}/>'
        )
        svg.append(f'    <circle cx="11" cy="{ry - 4}" r="3.2" fill="{style["color"]}"/>')
        svg.append(
            f'    <text x="32" y="{ry}" fill="#EFEDE2" font-size="12.5" class="sans-bold">{style["name"]}</text>'
        )
        svg.append(
            f'    <text x="32" y="{ry + 14}" fill="#6E8479" font-size="8.5" class="mono-label">MEDIAN {med_ms:.1f} MS · RSS {peak_rss:.0f} MB</text>'
        )
        svg.append(
            f'    <text x="318" y="{ry + 3}" text-anchor="end" fill="{style["color"]}" font-size="13" class="mono-num">{rps:.1f}</text>'
        )
        svg.append(
            f'    <text x="426" y="{ry + 3}" text-anchor="end" fill="#EFEDE2" font-size="12" class="mono-num">+{peak_growth:.1f} MB</text>'
        )
        svg.append(
            f'    <line x1="0" y1="{ry + 22}" x2="426" y2="{ry + 22}" stroke="#EFEDE2" stroke-opacity="0.08" stroke-width="1"/>'
        )
    svg.append("  </g>")

    # 4. Middle Tall Vertical Strip Window (Slide 16:9 - 21 / 26 / 27)
    # Holds the perspective horizon light-rays + Throughput comparison bars
    sx, sy, sw, sh = 520, 54, 156, 656
    svg.append(
        f"""  <g id="vertical-horizon-strip">
    <rect x="{sx}" y="{sy}" width="{sw}" height="{sh}" rx="10" fill="url(#emerald-burst)" stroke="#EFEDE2" stroke-opacity="0.7" stroke-width="1.3"/>
    <g clip-path="url(#strip-clip)">
      <!-- Perspective starburst rays at bottom of vertical strip (Slide 21 / 26 / 27) -->
      <circle cx="{sx + sw/2}" cy="{sy + sh - 110}" r="42" fill="#04D87F" opacity="0.28"/>
      <circle cx="{sx + sw/2}" cy="{sy + sh - 110}" r="14" fill="#EFEDE2" opacity="0.75"/>"""
    )
    vanish_x, vanish_y = sx + sw / 2, sy + sh - 110
    for angle in range(0, 360, 18):
        rad = math.radians(angle)
        x2 = vanish_x + 240 * math.cos(rad)
        y2 = vanish_y + 240 * math.sin(rad)
        svg.append(
            f'      <line x1="{vanish_x:.1f}" y1="{vanish_y:.1f}" x2="{x2:.1f}" y2="{y2:.1f}" stroke="#04D87F" stroke-opacity="0.32" stroke-width="1.1"/>'
        )
    # Perspective floor grid lines
    for step_y in range(0, 120, 18):
        gy = vanish_y + step_y
        svg.append(
            f'      <line x1="{sx}" y1="{gy}" x2="{sx + sw}" y2="{gy}" stroke="#CEF4E4" stroke-opacity="0.28" stroke-width="0.9"/>'
        )

    # Vertical Strip Header + Hardened Throughput Callouts
    svg.append(
        f"""      <text x="{sx + 16}" y="{sy + 28}" fill="#04D87F" font-size="9.5" class="mono-label">THROUGHPUT</text>
      <text x="{sx + 16}" y="{sy + 42}" fill="#8FA398" font-size="8.5" class="mono-label">REQ / SEC</text>"""
    )
    for idx, r in enumerate(results[:4]):
        s_key = r.get("short_label", r["config"])
        style = SERIES_STYLE.get(s_key, {"color": "#04D87F"})
        cy = sy + 88 + idx * 96
        rps = float(r.get("rps", 0.0))
        short_tag = s_key.replace("LOCKSTEP ", "")
        svg.append(
            f"""      <rect x="{sx + 14}" y="{cy}" width="{sw - 28}" height="74" fill="#030D09" fill-opacity="0.82" stroke="{style['color']}" stroke-opacity="0.55" stroke-width="1"/>
      <text x="{sx + 24}" y="{cy + 20}" fill="#8FA398" font-size="8" class="mono-label">{short_tag}</text>
      <text x="{sx + 24}" y="{cy + 50}" fill="{style['color']}" font-size="22" class="druk-word">{rps:.1f}</text>
      <text x="{sx + 24}" y="{cy + 64}" fill="#EFEDE2" font-size="8" class="mono-label">REQ/S VERIFIED</text>"""
        )

    svg.append("    </g>\n  </g>")

    # 5. Right Two Stacked Framed Windows (Slide 16:9 - 26 / 27):
    #    TOP WINDOW:    MEMORY ALLOCATED OVER TIME (MB)
    #    BOTTOM WINDOW: CPU UTILIZATION OVER TIME (%)
    wx = 700
    ww = 686
    wh = 318
    top_wy = 54
    bot_wy = 392

    def draw_line_chart(
        win_y: float,
        bg_grad: str,
        kicker: str,
        title: str,
        unit: str,
        metric_fn,
        y_max_override: float | None = None,
    ) -> None:
        chart_id = kicker.split()[0].lower().replace("&", "").replace(";", "")
        svg.append(
            f"""  <g id="chart-{chart_id}">
    <rect x="{wx}" y="{win_y}" width="{ww}" height="{wh}" rx="10" fill="url(#{bg_grad})" stroke="#EFEDE2" stroke-opacity="0.75" stroke-width="1.3"/>
    <text x="{wx + 24}" y="{win_y + 28}" fill="#04D87F" font-size="10" class="mono-label">▾▴▾ {kicker}</text>
    <text x="{wx + 24}" y="{win_y + 52}" fill="#EFEDE2" font-size="20" class="sans-bold">{title}</text>
    <text x="{wx + ww - 24}" y="{win_y + 28}" text-anchor="end" fill="#8FA398" font-size="9.5" class="mono-label">SAMPLING: 50MS /PROC TELEMETRY</text>"""
        )

        pad_l, pad_r, pad_t, pad_b = 64, 28, 72, 42
        plot_x = wx + pad_l
        plot_y = win_y + pad_t
        plot_w = ww - pad_l - pad_r
        plot_h = wh - pad_t - pad_b

        # Compute max X (duration) and max Y across all series
        all_t = [pt["t_sec"] for r in results for pt in r.get("timeseries", [])]
        max_t = max(all_t) if all_t else 2.0
        all_v = [metric_fn(pt, r) for r in results for pt in r.get("timeseries", [])]
        raw_max_v = max(all_v) if all_v else 100.0
        max_v = y_max_override if y_max_override else max(10.0, math.ceil(raw_max_v * 1.15 / 10.0) * 10.0)

        # Horizontal grid lines & Y-axis labels
        for i in range(5):
            frac = i / 4.0
            gy = plot_y + plot_h - frac * plot_h
            val = frac * max_v
            svg.append(
                f'    <line x1="{plot_x}" y1="{gy:.1f}" x2="{plot_x + plot_w}" y2="{gy:.1f}" stroke="#EFEDE2" stroke-opacity="0.12" stroke-width="1"/>'
            )
            svg.append(
                f'    <text x="{plot_x - 10}" y="{gy + 3.5:.1f}" text-anchor="end" fill="#8FA398" font-size="9.5" class="mono-num">{val:.0f}{unit}</text>'
            )

        # Vertical time grid lines & X-axis labels (0.0s .. max_t)
        for i in range(5):
            frac = i / 4.0
            gx = plot_x + frac * plot_w
            t_val = frac * max_t
            svg.append(
                f'    <line x1="{gx:.1f}" y1="{plot_y}" x2="{gx:.1f}" y2="{plot_y + plot_h}" stroke="#EFEDE2" stroke-opacity="0.08" stroke-width="1"/>'
            )
            svg.append(
                f'    <text x="{gx:.1f}" y="{plot_y + plot_h + 20}" text-anchor="middle" fill="#8FA398" font-size="9.5" class="mono-num">{t_val:.1f}s</text>'
            )

        # Plot each Nginx configuration time-series line
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
            # Endpoint marker
            ex, ey = coords[-1]
            svg.append(
                f'    <circle cx="{ex:.1f}" cy="{ey:.1f}" r="3.2" fill="{style["color"]}" stroke="#020604" stroke-width="1"/>'
            )

        svg.append("  </g>")

    # Top Window: Dynamic Memory Allocated Over Time (MB)
    draw_line_chart(
        win_y=top_wy,
        bg_grad="window-top-bg",
        kicker="HEAP &amp; QUARANTINE ALLOCATION OVER TIME",
        title="Dynamic Memory Allocated Over Time (+MB)",
        unit=" MB",
        metric_fn=lambda pt, _r: float(pt.get("mem_growth_mb", pt.get("mem_Growth_mb", 0.0))),
        y_max_override=15.0,
    )

    # Bottom Window: CPU Utilization Over Time (%)
    draw_line_chart(
        win_y=bot_wy,
        bg_grad="window-bot-bg",
        kicker="COMPUTE TELEMETRY OVER TIME",
        title="Multi-Worker CPU Utilization Over Time (%)",
        unit="%",
        metric_fn=lambda pt, _r: float(pt.get("cpu_pct", 0.0)),
    )

    # 6. Bottom Cream Banner Strip (Slide 16:9 - 156 / 170)
    svg.append(
        f"""  <g id="bottom-wordmark-strip">
    <text x="50" y="{H - 17}" fill="#12543B" font-size="48" class="druk-word">LOCKSTEP</text>
    <text x="{W - 54}" y="{H - 40}" text-anchor="end" fill="#090909" font-size="10.5" class="mono-label">
      16-BYTE SHADOW GRANULES · SIPHASH-2-4 PAC · 256-CHUNK QUARANTINE · RANDOMIZED SLABS · CFI
    </text>
    <text x="{W - 54}" y="{H - 22}" text-anchor="end" fill="#12543B" font-size="10.5" class="mono-label">
      ▾▴▾ 8/8 EXPLOIT PRIMITIVES KILLED · VERIFIED ON MULTI-THREADED NGINX (GLIBC &amp; MUSL)
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
