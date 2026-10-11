import * as esbuild from "esbuild";
import { cp, mkdir } from "node:fs/promises";
const watch = process.argv.includes("--watch");
await mkdir("dist", { recursive: true });
await cp("public", "dist", { recursive: true });
const options = {
  entryPoints: { app: "src/index.tsx", "app-style": "src/styles.css" },
  bundle: true,
  minify: !watch,
  sourcemap: true,
  target: ["es2022"],
  outdir: "dist",
  define: { "process.env.NODE_ENV": watch ? '"development"' : '"production"' },
};
// CSS and JS share the same failing build and watch lifecycle.
if (watch) {
  const context = await esbuild.context(options);
  await context.watch();
} else {
  await esbuild.build(options);
}
