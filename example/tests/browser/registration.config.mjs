import { defineConfig, devices } from "@playwright/test";
export default defineConfig({
  testDir: ".",
  testMatch: "registration.spec.mjs",
  workers: 1,
  timeout: 60_000,
  use: { baseURL: process.env.REGISTRATION_BASE_URL },
  projects: [
    { name: "js-enabled", use: { ...devices["Desktop Chrome"], javaScriptEnabled: true } },
    { name: "js-disabled", use: { ...devices["Desktop Chrome"], javaScriptEnabled: false } },
  ],
});
