/**
 * Golden-path: /teams page shows the team created during setup.
 *
 * Depends on setup.spec.ts having run first (shared DB state).
 * Run with: npm run test:e2e
 */
import { test, expect } from "@playwright/test";

const OWNER_EMAIL = "owner@example.arc";
const OWNER_PASSWORD = "S3cur3P@ssw0rd!";
const TEAM_NAME = "Acme Corp";

/** Reusable login helper. */
async function loginAsOwner(page: import("@playwright/test").Page) {
  await page.goto("/login");
  await page.getByLabel(/email/i).fill(OWNER_EMAIL);
  await page.getByLabel(/password/i).fill(OWNER_PASSWORD);
  await page.getByRole("button", { name: /sign in|log in|login/i }).click();
  await page.waitForURL(/\/sessions/, { timeout: 15_000 });
}

test.describe("Teams page", () => {
  test("navigating to /teams shows the team created during setup", async ({
    page,
  }) => {
    await loginAsOwner(page);

    await page.goto("/teams");

    // Page should render without server errors
    await expect(page.locator("body")).not.toContainText(
      /500|internal server error/i
    );

    // The team name from setup should appear somewhere on the page
    await expect(page.getByText(TEAM_NAME, { exact: false })).toBeVisible({
      timeout: 10_000,
    });
  });

  test("/teams page includes a member count or member list entry", async ({
    page,
  }) => {
    await loginAsOwner(page);
    await page.goto("/teams");

    // At minimum the owner is a member, so either a count ≥ 1 or the email
    // should appear.  We check for either — the exact UI shape is flexible.
    const ownerPresent = page.getByText(OWNER_EMAIL, { exact: false });
    const memberCount = page.getByText(/member/i);

    const found =
      (await ownerPresent.isVisible({ timeout: 5_000 }).catch(() => false)) ||
      (await memberCount.isVisible({ timeout: 5_000 }).catch(() => false));

    expect(found).toBe(true);
  });
});
