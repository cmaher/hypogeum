import { expect, test } from "@playwright/test";
import { execFileSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { resolve } from "node:path";

// Set by auth-stack.mjs. Kratos cookies are Secure; Chromium accepts them on
// http://localhost, which is why the origin is localhost rather than 127.0.0.1.
test.use({ baseURL: process.env.EXEMPLUM_SMOKE_BASE_URL });

function promote(username) {
  execFileSync(
    "cargo",
    ["run", "--locked", "-q", "-p", "exemplum-admin-cli", "--bin", "exemplum-admin", "--", "promote", username],
    {
      cwd: resolve(import.meta.dirname, "../.."),
      env: {
        ...process.env,
        EXEMPLUM_ENV_FILE: "",
        EXEMPLUM_ADMIN_DATABASE_URL: process.env.EXEMPLUM_SMOKE_ADMIN_DATABASE_URL,
      },
      stdio: ["ignore", "pipe", "inherit"],
    },
  );
}

async function signedInStatus(page) {
  return (await page.request.get("/api/hello/me")).status();
}

test("register, get approved, log out and log back in", async ({ page }, testInfo) => {
  const username = `smoke-${testInfo.project.name}-${randomBytes(4).toString("hex")}`;
  const displayName = `Smoke ${testInfo.project.name}`;
  // Random so Kratos' breached-password check passes.
  const password = randomBytes(18).toString("base64url");

  await page.goto("/registration");
  await expect(page).toHaveURL(/\/registration\?flow=/);
  await expect(page.getByRole("heading", { name: "Register" })).toBeVisible();
  await page.getByLabel("Username").fill(username);
  await page.getByLabel("Display name").fill(displayName);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Sign up" }).click();
  await expect(page).toHaveURL(/\/registration-pending$/);
  await expect(page.getByText(/Registration received/)).toBeVisible();
  // Pending accounts are not signed in as far as the API is concerned.
  expect(await signedInStatus(page)).toBe(401);

  promote(username);
  await page.goto("/");
  await expect(page.getByRole("heading", { name: "Hello World" })).toBeVisible();
  const me = await page.request.get("/api/hello/me");
  expect(me.status()).toBe(200);
  expect((await me.json()).message).toBe(`Hello, ${displayName}!`);

  await page.getByRole("button", { name: "Log out" }).click();
  await expect(page).toHaveURL(/\/login\?flow=/);
  await expect(page.getByRole("heading", { name: "Log in" })).toBeVisible();
  expect(await signedInStatus(page)).toBe(401);

  await page.getByLabel(/Username|ID/).fill(username);
  await page.getByLabel("Password").fill(password);
  await page.getByRole("button", { name: "Sign in" }).click();
  await expect(page).toHaveURL(/\/$/);
  await expect(page.getByRole("button", { name: "Log out" })).toBeVisible();
  expect(await signedInStatus(page)).toBe(200);
});
