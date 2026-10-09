import * as esbuild from "esbuild";
import { execSync } from "node:child_process";
import { performance } from "node:perf_hooks";

const isWatch = process.argv.includes("--watch");
const start = performance.now();

// 1. Build Tailwind CSS
try {
  execSync("npx @tailwindcss/cli -i src/styles.css -o dist/app.css --minify", { stdio: "inherit" });
} catch (err) {
  console.error("Failed to build CSS with tailwindcss:", err);
}

// 2. Build JavaScript / TypeScript with esbuild (Go)
const buildOptions = {
  entryPoints: ["src/index.tsx"],
  bundle: true,
  minify: !isWatch,
  sourcemap: true,
  target: ["es2022"],
  outfile: "dist/app.js",
  loader: {
    ".svg": "text",
    ".woff2": "copy",
    ".woff": "copy",
    ".otf": "copy",
  },
  define: {
    "process.env.NODE_ENV": isWatch ? '"development"' : '"production"',
  },
};

if (isWatch) {
  const ctx = await esbuild.context(buildOptions);
  await ctx.watch();
  console.log(`[esbuild:go] Watching for changes...`);
} else {
  await esbuild.build(buildOptions);
  const elapsed = (performance.now() - start).toFixed(1);
  console.log(`[esbuild:go + tailwind] Built full app in ${elapsed}ms -> dist/`);
}
