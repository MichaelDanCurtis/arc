/**
 * Golden-path: first-run setup wizard + login flow.
 *
 * Assumes a freshly migrated arc-relay (no owner yet).  If the DB already has
 * an owner the /setup page redirects away — run the stack with a clean volume
 * or wipe the `users` table between runs.
 */
import { test, expect } from "@playwright/test";

const OWNER_EMAIL = "owner@example.arc";
const OWNER_PASSWORD = "S3cur3P@ssw0rd!";
const TEAM_NAME = "Acme Corp";

test.describe("Setup wizard and login", () => {
  test("step 1 — /setup renders the owner-creation form", async ({ page }) => {
    await page.goto("/setup");

    // Expect a heading or label that signals the setup UI
    await expect(
      page.getByRole("heading", { name: /setup|create owner|get started/i })
    ).toBeVisible({ timeout: 10_000 });
  });

  test("step 2 — complete setup wizard creates owner + team", async ({
    page,
  }) => {
    await page.goto("/setup");

    // --- Owner credentials ---
    const emailInput = page.getByLabel(/email/i);
    const passwordInput = page.getByLabel(/^password$/i);

    await emailInput.fill(OWNER_EMAIL);
    await passwordInput.fill(OWNER_PASSWORD);

    // Some wizards have a confirm-password field
    const confirmInput = page.getByLabel(/confirm password/i);
    if (await confirmInput.isVisible()) {
      await confirmInput.fill(OWNER_PASSWORD);
    }

    // --- Team name (may be on same page or a next step) ---
    const teamInput = page.getByLabel(/team name/i);
    if (await teamInput.isVisible()) {
      await teamInput.fill(TEAM_NAME);
    }

    // Submit
    await page.getByRole("button", { name: /create|submit|finish|next/i }).click();

    // Multi-step wizard: fill team name on step 2 if not already visible
    const teamInputStep2 = page.getByLabel(/team name/i);
    if (await teamInputStep2.isVisible({ timeout: 3_000 }).catch(() => false)) {
      await teamInputStep2.fill(TEAM_NAME);
      await page.getByRole("button", { name: /create|submit|finish|next/i }).click();
    }

    // After setup completes we should land somewhere other than /setup
    await expect(page).not.toHaveURL(/\/setup/, { timeout: 15_000 });
  });

  test("step 3 — login with owner credentials redirects to /sessions", async ({
    page,
  }) => {
    await page.goto("/login");

    await page.getByLabel(/email/i).fill(OWNER_EMAIL);
    await page.getByLabel(/password/i).fill(OWNER_PASSWORD);
    await page.getByRole("button", { name: /sign in|log in|login/i }).click();

    // Should redirect to the sessions dashboard
    await expect(page).toHaveURL(/\/sessions/, { timeout: 15_000 });
  });

  test("step 4 — /sessions page shows session list (may be empty)", async ({
    page,
  }) => {
    await page.goto("/login");
    await page.getByLabel(/email/i).fill(OWNER_EMAIL);
    await page.getByLabel(/password/i).fill(OWNER_PASSWORD);
    await page.getByRole("button", { name: /sign in|log in|login/i }).click();
    await page.waitForURL(/\/sessions/, { timeout: 15_000 });

    // Page renders without error
    await expect(page.locator("body")).not.toContainText(/500|internal server error/i);
  });
});
