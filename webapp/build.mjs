import * as esbuild from "esbuild";
import { performance } from "node:perf_hooks";

const isWatch = process.argv.includes("--watch");
const start = performance.now();

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
  const result = await esbuild.build(buildOptions);
  const elapsed = (performance.now() - start).toFixed(1);
  console.log(`[esbuild:go] Built bundle in ${elapsed}ms -> dist/app.js`);
}
