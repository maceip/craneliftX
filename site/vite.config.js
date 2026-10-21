import { defineConfig } from "vite";

export default defineConfig({
  base: "/lockstep/",
  build: {
    rolldownOptions: {
      output: {
        manualChunks: (id) =>
          id.includes("node_modules/three/") ? "three" : undefined,
      },
    },
  },
});
