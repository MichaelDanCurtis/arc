/**
 * Smoke test: shadow-diff panel renders on the session detail page and
 * successfully round-trips to the relay.
 *
 * This test does NOT require a live beacon-hostd — it's sufficient that the
 * relay responds with 404/502 for a non-existent session. The panel must
 * surface that error state cleanly (no blank UI, no unhandled rejection).
 *
 * Assumes the setup.spec.ts has already run and created the owner account.
 * Run via: `docker compose up` + `npm run dev` + `npm run test:e2e`.
 */
import { test, expect } from "@playwright/test";

const OWNER_EMAIL = "owner@example.arc";
const OWNER_PASSWORD = "S3cur3P@ssw0rd!";

async function login(page: import("@playwright/test").Page) {
  await page.goto("/login");
  await page.getByLabel(/email/i).fill(OWNER_EMAIL);
  await page.getByLabel(/password/i).fill(OWNER_PASSWORD);
  await page.getByRole("button", { name: /sign in|log in|login/i }).click();
  await page.waitForURL(/\/sessions/, { timeout: 15_000 });
}

test.describe("Shadow diff panel", () => {
  test("toggle button renders on session detail page", async ({ page }) => {
    await login(page);

    // Navigate directly to a synthetic session route — the page is a client
    // component that renders unconditionally regardless of whether the session
    // actually exists in the relay.
    await page.goto("/sessions/smoke-test-session");

    await expect(page.getByTestId("shadow-diff-toggle")).toBeVisible({
      timeout: 10_000,
    });
  });

  test("clicking toggle opens the panel", async ({ page }) => {
    await login(page);
    await page.goto("/sessions/smoke-test-session");

    await page.getByTestId("shadow-diff-toggle").click();

    await expect(page.getByTestId("shadow-diff-panel")).toBeVisible();
    await expect(
      page.getByRole("button", { name: /refresh/i }),
    ).toBeVisible();
  });

  test("refresh surfaces relay error for unknown session", async ({ page }) => {
    await login(page);
    await page.goto("/sessions/smoke-test-session");

    await page.getByTestId("shadow-diff-toggle").click();
    await page.getByRole("button", { name: /refresh/i }).click();

    // The relay should 404 because the session isn't registered. The panel
    // must either surface that as an error OR show the (empty) diff if by
    // some fluke the session exists — either is an acceptable round-trip.
    const error = page.getByTestId("shadow-diff-error");
    const empty = page.getByTestId("shadow-diff-empty");
    const content = page.getByTestId("shadow-diff-content");

    await expect(async () => {
      const visible =
        (await error.isVisible().catch(() => false)) ||
        (await empty.isVisible().catch(() => false)) ||
        (await content.isVisible().catch(() => false));
      expect(visible).toBe(true);
    }).toPass({ timeout: 15_000 });
  });
});
