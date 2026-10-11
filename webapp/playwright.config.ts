import { defineConfig } from "@playwright/test";
export default defineConfig({
  testDir: "./tests",
  testMatch: "**/*.spec.ts",
  workers: 1,
  fullyParallel: false,
  timeout: 180_000,
  expect: { timeout: 15_000 },
  use: {
    baseURL: process.env.ISR_BASE_URL || "http://127.0.0.1:8878",
    actionTimeout: 15000,
    navigationTimeout: 15000,
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },
  reporter: [["list"], ["html", { open: "never" }]],
  webServer: process.env.ISR_BASE_URL
    ? undefined
    : {
        command: "exec ../.venv/bin/python tests/serve.py",
        gracefulShutdown: { signal: "SIGTERM", timeout: 10000 },
        url: "http://127.0.0.1:8878/api/health",
        reuseExistingServer: false,
        timeout: 30_000,
      },
});
