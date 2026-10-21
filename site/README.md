# Lockstep

The public site is configured for https://maceip.github.io/craneliftX/.

A two-column, scroll-led field guide pairs an operator or attacker terminal with
an abstract Three.js memory layout. Fourteen chapters cover a service, five
corruption patterns, shadow tags, quarantine, randomized placement, pointer
authentication, instruction boundaries, diversification, execution boundaries,
and evidence. On mobile the memory view stays above the scrolling narrative.

```sh
cd site
npm ci
npm run dev
# http://127.0.0.1:5173/craneliftX/
npm run build
npx playwright install chromium
npm test
```

`src/story.ts` owns the explanations and source links. `src/memory-scene.ts`
renders 96 instanced blocks. `src/main.ts` connects chapter visibility, terminal
typing, deep links, keyboard controls and the memory view. Each heap block is
8 bytes; the off-by-one scene shows a one-byte slice. Code scenes label their
own scale. These are teaching models, not live guest execution or literal
Nginx/glibc/musl object layouts. Implementation notes distinguish actual checks
from schematic outcomes and state their coverage limits.

IntersectionObserver follows the current chapter. Rendering runs only for
bounded scene transitions; hidden pages stop, reduced motion removes typing
and transitions, and a motion toggle is available. A colored DOM grid replaces
the canvas if WebGL is unavailable or lost. The test suite covers desktop and
mobile layouts, keyboard controls, forward/backward scrolling, accessibility,
assets, idle rendering, reduced motion, fallback and context restoration.

The supplied Kesan Graffiti font is self-hosted with its original license in
`public/fonts/`. That license says Freeware, Non-Commercial; commercial use
requires the font author's permission. No external asset API is needed.

The existing GitHub Actions workflow builds, browser-tests and deploys `dist/`
on main-branch site changes. The URL base remains `/craneliftX/`.
