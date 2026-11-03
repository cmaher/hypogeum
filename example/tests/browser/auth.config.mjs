import { defineConfig, devices } from "@playwright/test";

// Kratos login, registration and logout through Caddy against a real,
// isolated stack; see auth-stack.mjs.
export default defineConfig({
  testDir: ".",
  testMatch: "auth.spec.mjs",
  globalSetup: "./auth-stack.mjs",
  // Builds and stack startup happen in global setup, outside this timeout.
  timeout: 60_000,
  workers: 1,
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
});
