import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "./tests",
  timeout: 45_000,
  workers: 1,
  reporter: [["list"], ["html", { open: "never" }]],
  use: {
    baseURL: "http://127.0.0.1:4174/lockstep/",
    viewport: { width: 1440, height: 1100 },
    launchOptions: {
      args: ["--use-angle=swiftshader", "--enable-unsafe-swiftshader"],
    },
    trace: "retain-on-failure",
  },
  webServer: {
    command: "npm run preview -- --port 4174",
    url: "http://127.0.0.1:4174/lockstep/",
    reuseExistingServer: !process.env.CI,
  },
});
