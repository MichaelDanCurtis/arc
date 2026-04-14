/**
 * Golden-path: /audit page renders the setup event.
 *
 * After the setup wizard completes arc-relay emits an audit log entry.
 * This test verifies the arc-web UI surfaces it.
 *
 * Depends on setup.spec.ts having run first (shared DB state).
 */
import { test, expect } from "@playwright/test";

const OWNER_EMAIL = "owner@example.arc";
const OWNER_PASSWORD = "S3cur3P@ssw0rd!";

async function loginAsOwner(page: import("@playwright/test").Page) {
  await page.goto("/login");
  await page.getByLabel(/email/i).fill(OWNER_EMAIL);
  await page.getByLabel(/password/i).fill(OWNER_PASSWORD);
  await page.getByRole("button", { name: /sign in|log in|login/i }).click();
  await page.waitForURL(/\/sessions/, { timeout: 15_000 });
}

test.describe("Audit log page", () => {
  test("/audit is reachable and does not 500", async ({ page }) => {
    await loginAsOwner(page);
    await page.goto("/audit");

    await expect(page.locator("body")).not.toContainText(
      /500|internal server error/i
    );
  });

  test("/audit contains at least one entry after setup", async ({ page }) => {
    await loginAsOwner(page);
    await page.goto("/audit");

    // Give the page time to load async data
    await page.waitForLoadState("networkidle", { timeout: 15_000 });

    // The audit table / list should have at least one row.
    // We look for common row selectors; adjust if the UI uses different markup.
    const rows = page.locator("table tbody tr, [data-testid='audit-row'], li[data-audit]");
    await expect(rows.first()).toBeVisible({ timeout: 10_000 });
  });

  test("/audit shows a setup or owner-created event", async ({ page }) => {
    await loginAsOwner(page);
    await page.goto("/audit");
    await page.waitForLoadState("networkidle", { timeout: 15_000 });

    // Arc-relay records events like "setup_complete", "owner_created", or
    // similar.  We search case-insensitively for common keywords.
    const setupEntry = page.getByText(
      /setup|owner.created|owner_created|initial.setup/i
    );
    await expect(setupEntry.first()).toBeVisible({ timeout: 10_000 });
  });
});
