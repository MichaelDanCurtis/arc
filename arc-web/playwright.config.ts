import { defineConfig, devices } from "@playwright/test";

/**
 * Playwright configuration for arc-web E2E tests.
 * Tests run against the full docker-compose stack (arc-relay + postgres).
 * Start the stack before running: `docker compose -f ../arc-relay/docker-compose.yml up -d`
 * Then start arc-web: `npm run dev`
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: false, // sequential — tests share DB state
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: "list",

  use: {
    baseURL: process.env.BASE_URL ?? "http://localhost:3000",
    trace: "retain-on-failure",
    screenshot: "only-on-failure",
  },

  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
