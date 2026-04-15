/**
 * Phase 5 C4.6 — Playwright E2E: WASM block-list rendering on the /watch page.
 *
 * Integration approach: C — page.routeWebSocket()
 *   Playwright 1.59 supports routeWebSocket, so we intercept the relay WS URL
 *   (ws://localhost:9900/ws) and serve a scripted mirror stream entirely in
 *   the test process. No real relay or hostd is needed.
 *
 * Auth bypass:
 *   The (protected) layout checks localStorage for "arc_access_token" and then
 *   calls GET /api/users/me.  We:
 *     1. page.addInitScript — injects a fake token into localStorage before
 *        any page JS runs.
 *     2. page.route("/api/users/me", …) — intercepts the HTTP fetch and returns
 *        a synthetic UserProfile JSON so the auth hook resolves as authenticated.
 *
 * Byte-sequence source: constructed inline.
 *   Block 1 (heuristic): plain shell prompt line "admin@arc:~$ ls\r\n".
 *     The WasmBlockDetector fires heuristic PromptStart on the newline after
 *     a line ending with a sigil-space.  This block is closed synthetically
 *     when the osc133 sequence arrives.
 *   Block 2 (osc133): full OSC 133 A/B/C/D sequence with exit code 0.
 *     Sending these bytes mode-locks the detector, emits a synthetic
 *     heuristic CommandEnd for block 1, then opens and closes block 2.
 *
 * WASM readiness:
 *   The WASM module loads async after mount.  feedBytes() is a no-op until the
 *   detector resolves.  Rather than a fixed timeout, we use Playwright's built-in
 *   auto-retry: after injecting both byte sequences we await
 *     expect(page.getByTestId("watch-block")).toHaveCount(2)
 *   which retries for up to 10 seconds.  If WASM hasn't resolved yet the count
 *   stays at 0 and the assertion keeps retrying until it reaches 2.
 *   To trigger a re-send if the first injection raced the WASM init, a small
 *   helper re-sends the bytes via the mocked WS route every 500 ms until blocks
 *   appear (see injectLoop below).
 */

import { test, expect } from "@playwright/test";
import { encode as msgpackEncode } from "@msgpack/msgpack";

// ---------------------------------------------------------------------------
// Relay constants
// ---------------------------------------------------------------------------

const SESSION_ID = "e2e-wasm-test-session";
const RELAY_WS_URL = "**/ws**";        // glob: matches ws://localhost:9900/ws?token=…
const RELAY_BASE = "http://localhost:9900";
const FAKE_TOKEN = "e2e-fake-access-token";

// ---------------------------------------------------------------------------
// Byte fixtures
//
// Block 1 — heuristic
//   A single prompt line "admin@arc:~$ ls\r\n".  The WasmBlockDetector fires
//   HeuristicPromptStart on the newline after it sees "$ ".
//
// Block 2 — osc133
//   Full OSC 133 A (PromptStart) → prompt text → B (CommandStart) → command
//   text → C (OutputStart) → output → D;0 (CommandEnd, exit=0).
//   Arrival of OSC 133 A closes block 1 synthetically before opening block 2.
// ---------------------------------------------------------------------------

// prettier-ignore
const HEURISTIC_BYTES = new Uint8Array([
  // "admin@arc:~$ ls\r\n"
  97, 100, 109, 105, 110,  64,  97, 114,  99,  58, 126,  36,
  32, 108, 115,  13,  10,
]);

// prettier-ignore
const OSC133_BYTES = new Uint8Array([
  // ESC ] 133 ; A BEL   — PromptStart
   27,  93,  49,  51,  51,  59,  65,   7,
  // "user@host:~$ "       — prompt text
  117, 115, 101, 114,  64, 104, 111, 115, 116,  58, 126,  36,  32,
  // ESC ] 133 ; B BEL   — CommandStart
   27,  93,  49,  51,  51,  59,  66,   7,
  // "echo hello"          — command text
  101,  99, 104, 111,  32, 104, 101, 108, 108, 111,
  // ESC ] 133 ; C BEL   — OutputStart
   27,  93,  49,  51,  51,  59,  67,   7,
  // "hello\r\n"           — output
  104, 101, 108, 108, 111,  13,  10,
  // ESC ] 133 ; D ; 0 BEL — CommandEnd (exit=0)
   27,  93,  49,  51,  51,  59,  68,  59,  48,   7,
]);

// ---------------------------------------------------------------------------
// Helper: encode a SessionOutput ArcToClient envelope as msgpack.
// ---------------------------------------------------------------------------

function sessionOutputMsg(bytes: Uint8Array, seq: number): Buffer {
  const msg = {
    kind: "SessionOutput",
    data: {
      session_id: SESSION_ID,
      bytes,
      seq,
      replay: false,
    },
  };
  return Buffer.from(msgpackEncode(msg));
}

// ---------------------------------------------------------------------------
// Auth bypass helpers
// ---------------------------------------------------------------------------

/**
 * Inject a fake access token into localStorage before the page script runs.
 * This causes getAccessToken() to return a non-null value, which makes the
 * useWebSocket hook attempt a WS connection (intercepted below) rather than
 * immediately returning an error state.
 */
async function injectFakeToken(page: import("@playwright/test").Page) {
  await page.addInitScript((token) => {
    localStorage.setItem("arc_access_token", token);
  }, FAKE_TOKEN);
}

/**
 * Mock GET /api/users/me to return a synthetic UserProfile.
 * This unblocks the useAuth hook so isAuthenticated becomes true.
 */
async function mockUsersMe(page: import("@playwright/test").Page) {
  await page.route(`${RELAY_BASE}/api/users/me`, (route) => {
    route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({
        id: "e2e-user-1",
        email: "e2e@example.arc",
        name: "E2E User",
        role: "owner",
      }),
    });
  });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

test.describe("Watch page — WASM block-list (C4.6)", () => {
  test("block-list sidebar is visible and renders two blocks from scripted mirror stream", async ({
    page,
  }) => {
    // --- Auth setup (must happen before navigation) ---
    await injectFakeToken(page);
    await mockUsersMe(page);

    // --- WS route: intercept relay WS and serve scripted mirror stream ---
    //
    // We hold a reference to the WS route so we can send messages later.
    // The route handler fires when the page opens the WebSocket.
    let wsRoute: import("@playwright/test").WebSocketRoute | null = null;

    await page.routeWebSocket(RELAY_WS_URL, (ws) => {
      wsRoute = ws;

      // Swallow all client→server messages (SubscribeSession, UnsubscribeSession, etc.)
      ws.onMessage(() => {
        // Do not forward to server — we are the mock server.
      });

      // Do not close the socket on our side unless the test ends.
    });

    // --- Navigate to the watch page ---
    await page.goto(`/sessions/${SESSION_ID}/watch`);

    // --- Block-list sidebar must be visible (always rendered, regardless of WASM state) ---
    const sidebar = page.getByTestId("block-list-sidebar");
    await expect(sidebar).toBeVisible({ timeout: 10_000 });

    // --- Inject byte sequences via the mock WS route ---
    //
    // Strategy: wait for the WS route to be captured (wsRoute set above),
    // then send bytes in a polling loop.  Each iteration sends both sequences
    // and waits 500 ms.  Playwright's auto-retry on the count assertion stops
    // the loop as soon as two blocks appear.
    //
    // Why a loop rather than a single send?
    //   The WASM detector loads async after mount.  feedBytes() silently drops
    //   bytes arriving before init resolves.  Resending periodically guarantees
    //   the bytes are delivered to a live detector even on slow machines.

    const injectBytes = () => {
      if (!wsRoute) return;
      wsRoute.send(sessionOutputMsg(HEURISTIC_BYTES, 1));
      wsRoute.send(sessionOutputMsg(OSC133_BYTES, 2));
    };

    // Initial injection — may be a no-op if WASM hasn't loaded yet.
    injectBytes();

    // Polling loop: retry every 500 ms until two blocks appear (max 12 s).
    const interval = setInterval(injectBytes, 500);
    try {
      await expect(page.getByTestId("watch-block")).toHaveCount(2, {
        timeout: 12_000,
      });
    } finally {
      clearInterval(interval);
    }

    const blocks = page.getByTestId("watch-block");

    // --- Block 0: heuristic ---
    const block0 = blocks.nth(0);

    // Mode badge text is "heuristic"
    await expect(block0.getByText("heuristic")).toBeVisible();

    // Exit-code chip is em-dash (D4: heuristic blocks have no reliable exit code)
    await expect(block0.getByTestId("exit-code-chip")).toHaveText("—");

    // --- Block 1: osc133 ---
    const block1 = blocks.nth(1);

    // Mode badge text is "osc133"
    await expect(block1.getByText("osc133")).toBeVisible();

    // Exit-code chip is "0" with green styling
    const exitChip = block1.getByTestId("exit-code-chip");
    await expect(exitChip).toHaveText("0");
    // Green styling: the chip carries border-green-600 / text-green-600
    await expect(exitChip).toHaveClass(/green/);

    // Command text is non-empty for the osc133 block
    // (prompt bytes "user@host:~$ " are decoded; sigil+space stripped → "user@host:~")
    const commandText = block1.getByTestId("command-text");
    await expect(commandText).not.toHaveText("—");
  });

  test("collapse toggle narrows the block-list sidebar", async ({ page }) => {
    // --- Auth setup ---
    await injectFakeToken(page);
    await mockUsersMe(page);

    // Swallow the WS connection — no bytes needed for this test.
    await page.routeWebSocket(RELAY_WS_URL, (ws) => {
      ws.onMessage(() => {});
    });

    await page.goto(`/sessions/${SESSION_ID}/watch`);

    const sidebar = page.getByTestId("block-list-sidebar");
    await expect(sidebar).toBeVisible({ timeout: 10_000 });

    // Measure initial width (open state: w-72 = 18 rem ≈ 288 px)
    const widthBefore = await sidebar.evaluate(
      (el) => (el as HTMLElement).getBoundingClientRect().width,
    );

    // Click the collapse toggle
    await page.getByTestId("block-list-toggle").click();

    // After collapse the sidebar should be significantly narrower (w-8 = 2 rem)
    await expect(async () => {
      const widthAfter = await sidebar.evaluate(
        (el) => (el as HTMLElement).getBoundingClientRect().width,
      );
      expect(widthAfter).toBeLessThan(widthBefore / 2);
    }).toPass({ timeout: 5_000 });
  });
});
