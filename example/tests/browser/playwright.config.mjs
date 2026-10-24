import { defineConfig, devices } from "@playwright/test";

const port = process.env.SMOKE_PORT || "18080";

export default defineConfig({
  testDir: ".",
  testMatch: "*.spec.mjs",
  workers: 1,
  use: { baseURL: `http://127.0.0.1:${port}` },
  projects: [
    {
      name: "js-enabled",
      use: { ...devices["Desktop Chrome"], javaScriptEnabled: true },
    },
    {
      name: "js-disabled",
      use: { ...devices["Desktop Chrome"], javaScriptEnabled: false },
    },
  ],
  webServer: {
    command: `cargo run --locked -p exemplum-app --example view_smoke_server -- 127.0.0.1:${port}`,
    cwd: "../..",
    url: `http://127.0.0.1:${port}/healthz`,
    timeout: 600_000,
    reuseExistingServer: !process.env.CI,
  },
});
