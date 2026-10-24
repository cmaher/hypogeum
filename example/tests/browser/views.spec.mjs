import { expect, test } from "@playwright/test";

const VALIDATION_MESSAGE =
  "Names can have at most 200 characters and cannot contain NUL characters.";
const NETWORK_FAILURE =
  "The request could not be completed. Check your connection and try again.";

test.beforeEach(async ({ page, baseURL }) => {
  // Every asset is served locally; nothing is fetched from a CDN.
  page.on("request", (request) => {
    expect(request.url().startsWith(baseURL)).toBe(true);
  });
  await page.goto("/");
});

async function submit(page, name) {
  await page.getByLabel("Name").fill(name);
  await page.getByRole("button", { name: "Say hello" }).click();
}

test("greets with the entered name", async ({ page, javaScriptEnabled }) => {
  await submit(page, "Ada <b>");
  await expect(page.locator("[data-greeting]")).toHaveText("Hello, Ada <b>!");
  await expect(page.getByLabel("Name")).toHaveValue("Ada <b>");
  await expect(page.locator("b")).toHaveCount(0);
  // HTMX swaps the section in place; ordinary forms navigate to the action.
  await expect(page).toHaveURL(javaScriptEnabled ? /\/$/ : /\/hello$/);
});

test("blank names greet the world", async ({ page }) => {
  await submit(page, "   ");
  await expect(page.locator("[data-greeting]")).toHaveText("Hello, world!");
});

test("validation errors keep the entered value", async ({ page }) => {
  const name = "x".repeat(201);
  await submit(page, name);
  await expect(page.locator("#greeting-error")).toHaveText(VALIDATION_MESSAGE);
  await expect(page.getByLabel("Name")).toHaveValue(name);
  await expect(page.locator("[data-greeting]")).toHaveCount(0);
});

test("server failures are visible", async ({ page, javaScriptEnabled }) => {
  await submit(page, "server-error");
  if (javaScriptEnabled) {
    await expect(page.locator("[data-view-feedback]")).toHaveText(
      "Internal server error",
    );
    await expect(page.getByLabel("Name")).toHaveValue("server-error");
  } else {
    await expect(page.locator("[data-view-error]")).toHaveText(
      "Internal server error",
    );
  }
});

test("network failures are visible", async ({ page, javaScriptEnabled }) => {
  test.skip(!javaScriptEnabled, "without JavaScript the browser reports failures");
  await page.route("**/hello", (route) => route.abort("failed"));
  await submit(page, "Ada");
  await expect(page.locator("[data-view-feedback]")).toHaveText(NETWORK_FAILURE);
  await expect(page.getByLabel("Name")).toHaveValue("Ada");
});
