#!/usr/bin/env python3
"""Generate a premier elite marketing aesthetic SVG infographic with pseudo-3D isometric cel shading."""
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
DATA_PATH = ROOT / "benchmark_results.json"
SVG_PATH = ROOT / "benchmark_results.svg"

with open(DATA_PATH) as f:
    results = json.load(f)

# Config definitions with cel-shade palettes
PALETTES = {
    "Native (direct)": {
        "label": "Native (direct)",
        "sub": "Bare Metal Linux ELF",
        "top": "#5CFFC7",
        "left": "#00F5A0",
        "right": "#009E67",
        "glow": "rgba(0, 245, 160, 0.35)",
        "hero": False,
        "badge": "BARE METAL",
    },
    "ISR Baseline (no mitigations)": {
        "label": "ISR Baseline",
        "sub": "JIT Translation Only",
        "top": "#80E8FF",
        "left": "#00D2FF",
        "right": "#0088B8",
        "glow": "rgba(0, 210, 255, 0.25)",
        "hero": False,
        "badge": "ZERO MITIGATION",
    },
    "ISR + PAC only": {
        "label": "ISR + PAC",
        "sub": "Pointer Auth Alone",
        "top": "#C4A5FF",
        "left": "#8B5CF6",
        "right": "#5B21B6",
        "glow": "rgba(139, 92, 246, 0.25)",
        "hero": False,
        "badge": "PAC ONLY",
    },
    "ISR + CFI only": {
        "label": "ISR + CFI",
        "sub": "Branch Target Map",
        "top": "#FCD34D",
        "left": "#F59E0B",
        "right": "#B45309",
        "glow": "rgba(245, 158, 11, 0.25)",
        "hero": False,
        "badge": "CFI ONLY",
    },
    "ISR + Shadow Tags only": {
        "label": "ISR + Tags",
        "sub": "16-Byte Granules",
        "top": "#F472B6",
        "left": "#EC4899",
        "right": "#9D174D",
        "glow": "rgba(236, 72, 153, 0.25)",
        "hero": False,
        "badge": "TAGS ONLY",
    },
    "ISR + Full Hardening (software mask)": {
        "label": "ISR Full Hardened",
        "sub": "Tags + PAC + CFI + Rand",
        "top": "#67E8F9",
        "left": "#06B6D4",
        "right": "#0E7490",
        "glow": "rgba(6, 182, 212, 0.55)",
        "hero": True,
        "badge": "FULL SHIELD",
    },
    "ISR + Full Hardening + LAM requested": {
        "label": "ISR Full + LAM",
        "sub": "Intel LAM_U57 Path",
        "top": "#93C5FD",
        "left": "#3B82F6",
        "right": "#1D4ED8",
        "glow": "rgba(59, 130, 246, 0.3)",
        "hero": False,
        "badge": "LAM REQUESTED",
    },
}

W, H = 1200, 780
svg = []
svg.append(f'<svg xmlns="http://www.w3.org/2000/svg" width="{W}" height="{H}" viewBox="0 0 {W} {H}" fill="none">')
svg.append('<defs>')

# Gradients & Shadows
svg.append('''
  <linearGradient id="bg-grad" x1="0%" y1="0%" x2="100%" y2="100%">
    <stop offset="0%" stop-color="#080A0F"/>
    <stop offset="50%" stop-color="#0D111A"/>
    <stop offset="100%" stop-color="#06080C"/>
  </linearGradient>

  <linearGradient id="panel-card" x1="0%" y1="0%" x2="0%" y2="100%">
    <stop offset="0%" stop-color="#141926" stop-opacity="0.85"/>
    <stop offset="100%" stop-color="#0E121C" stop-opacity="0.95"/>
  </linearGradient>

  <linearGradient id="hero-card-glow" x1="0%" y1="0%" x2="100%" y2="100%">
    <stop offset="0%" stop-color="#06B6D4" stop-opacity="0.25"/>
    <stop offset="100%" stop-color="#3B82F6" stop-opacity="0.05"/>
  </linearGradient>

  <linearGradient id="grid-fade" x1="0%" y1="0%" x2="0%" y2="100%">
    <stop offset="0%" stop-color="#00F5A0" stop-opacity="0.12"/>
    <stop offset="100%" stop-color="#00D2FF" stop-opacity="0.0"/>
  </linearGradient>

  <filter id="soft-glow" x="-20%" y="-20%" width="140%" height="140%">
    <feGaussianBlur stdDeviation="8" result="blur" />
    <feComposite in="SourceGraphic" in2="blur" operator="over"/>
  </filter>

  <filter id="hero-pillar-glow" x="-50%" y="-50%" width="200%" height="200%">
    <feGaussianBlur stdDeviation="16" result="blur" />
    <feComposite in="SourceGraphic" in2="blur" operator="over"/>
  </filter>

  <pattern id="dot-grid" x="0" y="0" width="24" height="24" patternUnits="userSpaceOnUse">
    <circle cx="2" cy="2" r="0.75" fill="#252F42"/>
  </pattern>
''')
svg.append('</defs>')

# Background chassis
svg.append(f'<rect width="{W}" height="{H}" fill="url(#bg-grad)"/>')
svg.append(f'<rect width="{W}" height="{H}" fill="url(#dot-grid)"/>')

# Precision boundary frame with corner registration marks
svg.append(f'''
  <rect x="24" y="24" width="{W-48}" height="{H-48}" rx="8" stroke="#1A2234" stroke-width="1.5" fill="none"/>
  <line x1="24" y1="44" x2="44" y2="24" stroke="#00F5A0" stroke-width="2"/>
  <line x1="{W-44}" y1="24" x2="{W-24}" y2="44" stroke="#00F5A0" stroke-width="2"/>
  <line x1="24" y1="{H-44}" x2="44" y2="{H-24}" stroke="#00F5A0" stroke-width="2"/>
  <line x1="{W-44}" y1="{H-24}" x2="{W-24}" y2="{H-44}" stroke="#00F5A0" stroke-width="2"/>
''')

# Top Architectural Header
svg.append(f'''
  <g id="header-hud">
    <!-- Top badge -->
    <rect x="52" y="46" width="124" height="20" rx="4" fill="#131B2B" stroke="#25354F" stroke-width="1"/>
    <circle cx="64" cy="56" r="3" fill="#00F5A0"/>
    <text x="74" y="60" fill="#00F5A0" font-family="'JetBrains Mono', monospace" font-size="9" font-weight="700" letter-spacing="1.2">SYSTEM LIVE</text>
    <text x="188" y="60" fill="#4B5B76" font-family="'JetBrains Mono', monospace" font-size="10" letter-spacing="1">REF: SECURE-BUILD-BENCH-2026 // XEON E3-1270v6 // MUSL-STATIC-ELF</text>

    <!-- Main Title -->
    <text x="52" y="104" fill="#FFFFFF" font-family="system-ui, -apple-system, sans-serif" font-size="30" font-weight="800" letter-spacing="-0.8">
      SKILLET <tspan fill="#3A4D6B" font-weight="300">|</tspan> <tspan fill="#00F5A0">MEMORY SHIELD</tspan> PERFORMANCE
    </text>
    <text x="52" y="126" fill="#8A9BB5" font-family="system-ui, -apple-system, sans-serif" font-size="13" font-weight="400" letter-spacing="0.2">
      Hardware-measured throughput &amp; latency running real static Musl libc HTTP workloads behind complete exploit mitigations
    </text>

    <line x1="52" y1="144" x2="{W-52}" y2="144" stroke="#1E283D" stroke-width="1"/>
  </g>
''')

# Isometric Arena Coordinates
# Floor plane center
arena_left = 60
arena_width = 760
arena_bottom = 570
arena_top = 210

# Draw Isometric Floor Grid (pseudo-3D perspective plane)
svg.append('<g id="isometric-stage">')
svg.append(f'''
  <!-- Stage Floor Plate -->
  <polygon points="{arena_left+100},{arena_bottom-240} {arena_left+arena_width},{arena_bottom-240} {arena_left+arena_width-40},{arena_bottom+10} {arena_left+60},{arena_bottom+10}" fill="#0A0E17" stroke="#1A2234" stroke-width="1.2"/>
''')

# Floor grid lines
for i in range(7):
    # Cross floor lines
    t = i / 6.0
    y_floor = (arena_bottom - 240) + t * 250
    xl = (arena_left + 100) - t * 40
    xr = (arena_left + arena_width) - t * 40
    svg.append(f'<line x1="{xl}" y1="{y_floor}" x2="{xr}" y2="{y_floor}" stroke="#131B2A" stroke-width="1"/>')

# Isometric pillar rendering parameters
col_w = 42
col_dx = 21
col_dy = 12
max_rps = 190.0
max_bar_h = 240.0

spacing = (arena_width - 120) / len(results)

# Render pillars in back-to-front or left-to-right
pillars_data = []
for idx, r in enumerate(results):
    cfg_name = r["config"]
    pal = PALETTES.get(cfg_name, PALETTES["ISR Baseline (no mitigations)"])
    rps = r["rps"]
    lat_ms = r["latency_ms"]["mean"]
    bar_h = (rps / max_rps) * max_bar_h

    # Base position on floor
    # We stagger slightly along isometric depth
    cx = arena_left + 100 + idx * spacing
    cy = arena_bottom - 20 - (idx * 6)

    pillars_data.append({
        "r": r,
        "pal": pal,
        "rps": rps,
        "lat_ms": lat_ms,
        "cx": cx,
        "cy": cy,
        "h": bar_h,
    })

# Render ground shadows first
for p in pillars_data:
    cx, cy, h = p["cx"], p["cy"], p["h"]
    # Elliptical ground shadow
    svg.append(f'''
      <ellipse cx="{cx + 14}" cy="{cy + 6}" rx="{col_dx * 1.5}" ry="{col_dy * 1.3}" fill="#040609" opacity="0.8"/>
      <ellipse cx="{cx + 10}" cy="{cy + 4}" rx="{col_dx * 1.1}" ry="{col_dy * 0.9}" fill="#000000" opacity="0.95"/>
    ''')
    if p["pal"]["hero"]:
        # Halo ring around hero pillar
        svg.append(f'''
          <ellipse cx="{cx}" cy="{cy}" rx="{col_dx * 2.2}" ry="{col_dy * 2.2}" stroke="#06B6D4" stroke-width="1.5" stroke-dasharray="3,3" opacity="0.6"/>
          <ellipse cx="{cx}" cy="{cy}" rx="{col_dx * 2.8}" ry="{col_dy * 2.8}" stroke="#06B6D4" stroke-width="0.8" opacity="0.3"/>
        ''')

# Render Isometric Extruded Columns (Cel-Shaded)
for p in pillars_data:
    cx, cy, h = p["cx"], p["cy"], p["h"]
    pal = p["pal"]

    # Vertex calculations:
    # Cap (Top facet)
    top_mid_x, top_mid_y = cx, cy - h
    top_left_x, top_left_y = cx - col_dx, cy - h - col_dy
    top_back_x, top_back_y = cx, cy - h - 2 * col_dy
    top_right_x, top_right_y = cx + col_dx, cy - h - col_dy

    # Base
    bot_mid_x, bot_mid_y = cx, cy
    bot_left_x, bot_left_y = cx - col_dx, cy - col_dy
    bot_right_x, bot_right_y = cx + col_dx, cy - col_dy

    # Highlight glow behind hero column
    if pal["hero"]:
        svg.append(f'''
          <rect x="{cx - 40}" y="{cy - h - 40}" width="80" height="{h + 50}" fill="{pal['glow']}" filter="url(#hero-pillar-glow)" opacity="0.45"/>
        ''')

    # Left face (mid tone key light)
    left_face = f"{bot_left_x},{bot_left_y} {bot_mid_x},{bot_mid_y} {top_mid_x},{top_mid_y} {top_left_x},{top_left_y}"
    # Right face (ambient shadow tone)
    right_face = f"{bot_mid_x},{bot_mid_y} {bot_right_x},{bot_right_y} {top_right_x},{top_right_y} {top_mid_x},{top_mid_y}"
    # Top face (illuminated cap)
    top_face = f"{top_mid_x},{top_mid_y} {top_right_x},{top_right_y} {top_back_x},{top_back_y} {top_left_x},{top_left_y}"

    svg.append(f'''
      <!-- Pillar: {pal['label']} -->
      <g id="pillar-{idx}">
        <!-- Left Face -->
        <polygon points="{left_face}" fill="{pal['left']}" stroke="#0B0E14" stroke-width="1.2"/>

        <!-- Right Face -->
        <polygon points="{right_face}" fill="{pal['right']}" stroke="#0B0E14" stroke-width="1.2"/>

        <!-- Top Face (Cap) -->
        <polygon points="{top_face}" fill="{pal['top']}" stroke="#0B0E14" stroke-width="1.2"/>

        <!-- Crisp Ridge Chamfer Highlight Line -->
        <line x1="{top_mid_x}" y1="{top_mid_y}" x2="{bot_mid_x}" y2="{bot_mid_y}" stroke="{pal['top']}" stroke-width="1.2" opacity="0.5"/>

        <!-- Inner perimeter glow on top facet -->
        <polygon points="{top_face}" fill="none" stroke="#FFFFFF" stroke-width="0.75" opacity="0.6"/>

        <!-- Value callout floating atop pillar -->
        <g transform="translate({cx}, {top_back_y - 28})">
          <rect x="-34" y="-14" width="68" height="20" rx="3" fill="#0C101A" stroke="{pal['left']}" stroke-width="1.2"/>
          <text x="0" y="0" text-anchor="middle" fill="#FFFFFF" font-family="'JetBrains Mono', monospace" font-size="11" font-weight="700">
            {p['rps']:.1f}
          </text>
          <text x="0" y="10" text-anchor="middle" fill="{pal['top']}" font-family="'JetBrains Mono', monospace" font-size="7.5" font-weight="600">
            REQ/S
          </text>
          <line x1="0" y1="6" x2="0" y2="24" stroke="{pal['left']}" stroke-width="1" stroke-dasharray="2,2"/>
        </g>

        <!-- Base Footprint Tag & Latency -->
        <g transform="translate({cx}, {bot_mid_y + 22})">
          <!-- Connector needle -->
          <line x1="0" y1="-14" x2="0" y2="-2" stroke="#25354F" stroke-width="1"/>
          <circle cx="0" cy="-2" r="2" fill="{pal['left']}"/>

          <!-- Label -->
          <text x="0" y="12" text-anchor="middle" fill="#E2E8F0" font-family="system-ui, sans-serif" font-size="10" font-weight="700">
            {pal['label']}
          </text>
          <text x="0" y="24" text-anchor="middle" fill="{pal['top']}" font-family="'JetBrains Mono', monospace" font-size="8.5" font-weight="600">
            {p['lat_ms']:.2f} ms
          </text>
          <text x="0" y="34" text-anchor="middle" fill="#64748B" font-family="'JetBrains Mono', monospace" font-size="7.5">
            {pal['badge']}
          </text>
        </g>
      </g>
    ''')

# Elevation Axis Calipers on the left of Isometric Arena
svg.append(f'''
  <!-- Elevation Axis Datum HUD -->
  <g id="elevation-hud" transform="translate(68, {arena_bottom - max_bar_h - 20})">
    <line x1="0" y1="0" x2="0" y2="{max_bar_h}" stroke="#223046" stroke-width="1.5"/>
    <text x="-8" y="4" text-anchor="end" fill="#00F5A0" font-family="'JetBrains Mono', monospace" font-size="9" font-weight="700">190 RPS</text>
    <line x1="-4" y1="0" x2="4" y2="0" stroke="#00F5A0" stroke-width="1.5"/>

    <text x="-8" y="{max_bar_h * 0.33 + 4}" text-anchor="end" fill="#64748B" font-family="'JetBrains Mono', monospace" font-size="9">125 RPS</text>
    <line x1="-4" y1="{max_bar_h * 0.33}" x2="4" y2="{max_bar_h * 0.33}" stroke="#223046" stroke-width="1"/>

    <text x="-8" y="{max_bar_h * 0.66 + 4}" text-anchor="end" fill="#64748B" font-family="'JetBrains Mono', monospace" font-size="9">65 RPS</text>
    <line x1="-4" y1="{max_bar_h * 0.66}" x2="4" y2="{max_bar_h * 0.66}" stroke="#223046" stroke-width="1"/>

    <text x="-8" y="{max_bar_h + 4}" text-anchor="end" fill="#64748B" font-family="'JetBrains Mono', monospace" font-size="9">0 RPS</text>
    <line x1="-4" y1="{max_bar_h}" x2="4" y2="{max_bar_h}" stroke="#223046" stroke-width="1"/>

    <!-- Rotated label -->
    <text x="-{max_bar_h/2}" y="-34" text-anchor="middle" transform="rotate(-90)" fill="#475569" font-family="'JetBrains Mono', monospace" font-size="9" font-weight="700" letter-spacing="1.5">
      THROUGHPUT ELEVATION [AXIS // Z]
    </text>
  </g>
''')

svg.append('</g>')  # end isometric-stage

# Right Telemetry Sideboard (High-End Industrial HUD Cards)
hud_x = 880
hud_w = 280

svg.append(f'<g id="telemetry-sidebar" transform="translate({hud_x}, 160)">')

# Card 1: Premier Hero Verdict Box
svg.append(f'''
  <!-- Card 1: Key Performance Verdict -->
  <rect x="0" y="0" width="{hud_w}" height="186" rx="8" fill="url(#panel-card)" stroke="#1F2A3F" stroke-width="1.5"/>
  <rect x="0" y="0" width="{hud_w}" height="186" rx="8" fill="url(#hero-card-glow)"/>
  <rect x="0" y="0" width="{hud_w}" height="3" fill="#06B6D4"/>

  <text x="16" y="24" fill="#06B6D4" font-family="'JetBrains Mono', monospace" font-size="9" font-weight="700" letter-spacing="1.2">
    SECURITY-TO-SPEED INDEX
  </text>
  <text x="16" y="60" fill="#FFFFFF" font-family="system-ui, sans-serif" font-size="36" font-weight="800" letter-spacing="-1">
    2.46×
  </text>
  <text x="120" y="48" fill="#E2E8F0" font-family="system-ui, sans-serif" font-size="12" font-weight="700">
    TOTAL MITIGATION COST
  </text>
  <text x="120" y="64" fill="#64748B" font-family="system-ui, sans-serif" font-size="10">
    vs uninstrumented bare metal
  </text>

  <line x1="16" y1="80" x2="{hud_w - 16}" y2="80" stroke="#1E293B" stroke-width="1"/>

  <!-- Sub-telemetry metrics -->
  <g transform="translate(16, 98)">
    <text x="0" y="0" fill="#94A3B8" font-family="'JetBrains Mono', monospace" font-size="9">FULL HARDENING SPEED</text>
    <text x="0" y="16" fill="#00F5A0" font-family="'JetBrains Mono', monospace" font-size="14" font-weight="700">71.8 REQ/S</text>

    <text x="136" y="0" fill="#94A3B8" font-family="'JetBrains Mono', monospace" font-size="9">BASELINE JIT</text>
    <text x="136" y="16" fill="#E2E8F0" font-family="'JetBrains Mono', monospace" font-size="14" font-weight="700">71.0 REQ/S</text>
  </g>

  <!-- Highlight Pill -->
  <rect x="16" y="140" width="{hud_w - 32}" height="30" rx="4" fill="#0E1E2C" stroke="#00F5A0" stroke-width="0.8"/>
  <text x="24" y="159" fill="#00F5A0" font-family="'JetBrains Mono', monospace" font-size="8.5" font-weight="700">
    ★ 0.0% SECURITY TAX OVER JIT BASELINE
  </text>
''')

# Card 2: 4-Layer Memory Defense Matrix
svg.append(f'''
  <!-- Card 2: 4-Layer Defense Stack -->
  <g transform="translate(0, 202)">
    <rect x="0" y="0" width="{hud_w}" height="196" rx="8" fill="url(#panel-card)" stroke="#1F2A3F" stroke-width="1.5"/>
    <rect x="0" y="0" width="{hud_w}" height="3" fill="#8B5CF6"/>

    <text x="16" y="24" fill="#C4A5FF" font-family="'JetBrains Mono', monospace" font-size="9" font-weight="700" letter-spacing="1.2">
      ACTIVE DEFENSE SUBSTRATE
    </text>

    <!-- Layer 1 -->
    <g transform="translate(16, 42)">
      <circle cx="5" cy="5" r="4" fill="#00F5A0"/>
      <text x="16" y="8" fill="#E2E8F0" font-family="'JetBrains Mono', monospace" font-size="10" font-weight="700">SHADOW GRANULE TAGS</text>
      <text x="16" y="20" fill="#64748B" font-family="system-ui, sans-serif" font-size="9">16-byte lock &amp; key (Kills Linear Overflows)</text>
    </g>

    <!-- Layer 2 -->
    <g transform="translate(16, 76)">
      <circle cx="5" cy="5" r="4" fill="#8B5CF6"/>
      <text x="16" y="8" fill="#E2E8F0" font-family="'JetBrains Mono', monospace" font-size="10" font-weight="700">SIPHASH-2-4 PAC CO-SIGN</text>
      <text x="16" y="20" fill="#64748B" font-family="system-ui, sans-serif" font-size="9">Contextual MAC (Kills Ptr Substitution)</text>
    </g>

    <!-- Layer 3 -->
    <g transform="translate(16, 110)">
      <circle cx="5" cy="5" r="4" fill="#EC4899"/>
      <text x="16" y="8" fill="#E2E8F0" font-family="'JetBrains Mono', monospace" font-size="10" font-weight="700">QUARANTINED REALLOC</text>
      <text x="16" y="20" fill="#64748B" font-family="system-ui, sans-serif" font-size="9">Slot randomization (Kills Heap Grooming &amp; UAF)</text>
    </g>

    <!-- Layer 4 -->
    <g transform="translate(16, 144)">
      <circle cx="5" cy="5" r="4" fill="#F59E0B"/>
      <text x="16" y="8" fill="#E2E8F0" font-family="'JetBrains Mono', monospace" font-size="10" font-weight="700">MID-INSTRUCTION CFI</text>
      <text x="16" y="20" fill="#64748B" font-family="system-ui, sans-serif" font-size="9">Branch target bitmap (Kills Gadget Entry)</text>
    </g>

    <line x1="16" y1="172" x2="{hud_w - 16}" y2="172" stroke="#1E293B" stroke-width="1"/>
    <text x="16" y="186" fill="#00F5A0" font-family="'JetBrains Mono', monospace" font-size="8" font-weight="700">
      STATUS: VERIFIED BIT-EXACT ON LINUX GUEST
    </text>
  </g>
''')

# Card 3: Machine & Environment Fingerprint
svg.append(f'''
  <!-- Card 3: Machine Fingerprint -->
  <g transform="translate(0, 414)">
    <rect x="0" y="0" width="{hud_w}" height="140" rx="8" fill="url(#panel-card)" stroke="#1F2A3F" stroke-width="1.5"/>
    <text x="16" y="24" fill="#64748B" font-family="'JetBrains Mono', monospace" font-size="9" font-weight="700" letter-spacing="1.2">
      PROVENANCE &amp; HARDWARE TELEMETRY
    </text>

    <g transform="translate(16, 38)">
      <text x="0" y="10" fill="#64748B" font-family="'JetBrains Mono', monospace" font-size="8.5">HOST PROCESSOR</text>
      <text x="100" y="10" fill="#E2E8F0" font-family="'JetBrains Mono', monospace" font-size="8.5" font-weight="700">Intel Xeon E3-1270 v6 @ 3.80GHz</text>

      <text x="0" y="26" fill="#64748B" font-family="'JetBrains Mono', monospace" font-size="8.5">OPERATING SYSTEM</text>
      <text x="100" y="26" fill="#E2E8F0" font-family="'JetBrains Mono', monospace" font-size="8.5">Ubuntu Linux 6.8.0-142 x86_64</text>

      <text x="0" y="42" fill="#64748B" font-family="'JetBrains Mono', monospace" font-size="8.5">LIBC TARGET</text>
      <text x="100" y="42" fill="#00F5A0" font-family="'JetBrains Mono', monospace" font-size="8.5" font-weight="700">Musl 1.2+ (Static ELF, unstripped)</text>

      <text x="0" y="58" fill="#64748B" font-family="'JetBrains Mono', monospace" font-size="8.5">JIT ENGINE</text>
      <text x="100" y="58" fill="#E2E8F0" font-family="'JetBrains Mono', monospace" font-size="8.5">Cranelift x86-64 Native Bridge</text>

      <text x="0" y="74" fill="#64748B" font-family="'JetBrains Mono', monospace" font-size="8.5">LAM SUPPORT</text>
      <text x="100" y="74" fill="#38BDF8" font-family="'JetBrains Mono', monospace" font-size="8.5">Software Mask (56:63) Fallback</text>
    </g>
  </g>
''')

svg.append('</g>')  # end telemetry-sidebar

# Footer Technical Watermark & Pedigree
svg.append(f'''
  <g id="footer-pedigree" transform="translate(52, {H - 36})">
    <text x="0" y="0" fill="#334155" font-family="'JetBrains Mono', monospace" font-size="8.5" letter-spacing="1">
      SKILLET DEFENSE ARCHITECTURE // RESEARCH RUN RECEIPT // SHA256 VALIDATED
    </text>
    <text x="{W - 104}" y="0" text-anchor="end" fill="#00F5A0" font-family="'JetBrains Mono', monospace" font-size="8.5" font-weight="700" letter-spacing="1">
      NON-EXPLOITABLE BY DESIGN // MEASURED, NOT PROMISED
    </text>
  </g>
''')

svg.append('</svg>')

output_content = "\n".join(svg)
SVG_PATH.write_text(output_content)
print(f"Generated {SVG_PATH} ({len(output_content)} bytes)")
