import { expect, test } from "@playwright/test";

test("register and approve a pending account", async ({ page }, testInfo) => {
  const name = `Browser ${testInfo.project.name}`;
  await expect.poll(async () => (await page.request.get("/healthz")).status()).toBe(200);
  await page.goto("/register");
  await page.getByLabel("Display name").fill(name);
  await page.getByLabel("Username").fill(`browser-${testInfo.project.name}`);
  await page.getByLabel("Password").fill("browser-password-123");
  await page.getByRole("button", { name: "Register", exact: true }).click();
  await expect(page).toHaveURL(/registration-pending$/);
  await expect(page.getByText(/Registration received/)).toBeVisible();
  await page.goto("/login");
  await page.getByLabel("Username").fill("admin");
  await page.getByLabel("Password").fill("admin-password-123");
  await page.getByRole("button", { name: "Log in", exact: true }).click();
  await expect(page).toHaveURL(/\/$/);
  await page.goto("/admin/users");
  const row = page.getByRole("row").filter({ hasText: name });
  await expect(row).toContainText("Pending");
  await row.getByRole("button", { name: "Approve", exact: true }).click();
  await expect(row).toContainText("Approved");
  await expect(row.getByRole("button", { name: "Approve", exact: true })).toHaveCount(0);
});
