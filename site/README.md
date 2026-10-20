# The Heap Workshop

The repository's static, Three.js teaching site: https://maceip.github.io/craneliftX/

```sh
cd site
npm ci
npm run dev
# http://127.0.0.1:5173/craneliftX/
npm run build
npx playwright install chromium
npm test
```

`src/lessons.ts` contains the five sourced teaching scenarios. `src/workbench.ts`
builds the procedural models; no external asset service or runtime backend is
required. `src/main.ts` coordinates the scene, text, memory diagram, and playback.
The byte layouts and 8-bit size counter are explicitly schematic. The site does
not execute guest binaries or claim that its illustrations are runtime evidence.

Motion begins on reader interaction, stops offscreen or when the tab is hidden,
and is disabled for reduced-motion users. Text and byte diagrams remain usable
if WebGL fails. With JavaScript disabled, a static five-scenario summary remains.

GitHub Actions builds, checks all lesson states and desktop/mobile accessibility,
and deploys `dist/` to Pages on changes to `site/` on `main`. The `/craneliftX/`
base in `vite.config.js` matches the repository Pages URL. Self-hosted fonts and
their licenses are in `public/fonts`; the Three.js license is in
`public/THREE-LICENSE.txt`.
