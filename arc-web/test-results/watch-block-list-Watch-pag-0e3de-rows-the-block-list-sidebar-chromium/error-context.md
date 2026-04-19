# Instructions

- Following Playwright test failed.
- Explain why, be concise, respect Playwright best practices.
- Provide a snippet of code with the fix, if possible.

# Test info

- Name: watch-block-list.spec.ts >> Watch page — WASM block-list (C4.6) >> collapse toggle narrows the block-list sidebar
- Location: e2e/watch-block-list.spec.ts:236:7

# Error details

```
Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:3000/sessions/e2e-wasm-test-session/watch
Call log:
  - navigating to "http://localhost:3000/sessions/e2e-wasm-test-session/watch", waiting until "load"

```

# Test source

```ts
  146 |     page,
  147 |   }) => {
  148 |     // --- Auth setup (must happen before navigation) ---
  149 |     await injectFakeToken(page);
  150 |     await mockUsersMe(page);
  151 | 
  152 |     // --- WS route: intercept relay WS and serve scripted mirror stream ---
  153 |     //
  154 |     // We hold a reference to the WS route so we can send messages later.
  155 |     // The route handler fires when the page opens the WebSocket.
  156 |     let wsRoute: import("@playwright/test").WebSocketRoute | null = null;
  157 | 
  158 |     await page.routeWebSocket(RELAY_WS_URL, (ws) => {
  159 |       wsRoute = ws;
  160 | 
  161 |       // Swallow all client→server messages (SubscribeSession, UnsubscribeSession, etc.)
  162 |       ws.onMessage(() => {
  163 |         // Do not forward to server — we are the mock server.
  164 |       });
  165 | 
  166 |       // Do not close the socket on our side unless the test ends.
  167 |     });
  168 | 
  169 |     // --- Navigate to the watch page ---
  170 |     await page.goto(`/sessions/${SESSION_ID}/watch`);
  171 | 
  172 |     // --- Block-list sidebar must be visible (always rendered, regardless of WASM state) ---
  173 |     const sidebar = page.getByTestId("block-list-sidebar");
  174 |     await expect(sidebar).toBeVisible({ timeout: 10_000 });
  175 | 
  176 |     // --- Inject byte sequences via the mock WS route ---
  177 |     //
  178 |     // Strategy: wait for the WS route to be captured (wsRoute set above),
  179 |     // then send bytes in a polling loop.  Each iteration sends both sequences
  180 |     // and waits 500 ms.  Playwright's auto-retry on the count assertion stops
  181 |     // the loop as soon as two blocks appear.
  182 |     //
  183 |     // Why a loop rather than a single send?
  184 |     //   The WASM detector loads async after mount.  feedBytes() silently drops
  185 |     //   bytes arriving before init resolves.  Resending periodically guarantees
  186 |     //   the bytes are delivered to a live detector even on slow machines.
  187 | 
  188 |     const injectBytes = () => {
  189 |       if (!wsRoute) return;
  190 |       wsRoute.send(sessionOutputMsg(HEURISTIC_BYTES, 1));
  191 |       wsRoute.send(sessionOutputMsg(OSC133_BYTES, 2));
  192 |     };
  193 | 
  194 |     // Initial injection — may be a no-op if WASM hasn't loaded yet.
  195 |     injectBytes();
  196 | 
  197 |     // Polling loop: retry every 500 ms until two blocks appear (max 12 s).
  198 |     const interval = setInterval(injectBytes, 500);
  199 |     try {
  200 |       await expect(page.getByTestId("watch-block")).toHaveCount(2, {
  201 |         timeout: 12_000,
  202 |       });
  203 |     } finally {
  204 |       clearInterval(interval);
  205 |     }
  206 | 
  207 |     const blocks = page.getByTestId("watch-block");
  208 | 
  209 |     // --- Block 0: heuristic ---
  210 |     const block0 = blocks.nth(0);
  211 | 
  212 |     // Mode badge text is "heuristic"
  213 |     await expect(block0.getByText("heuristic")).toBeVisible();
  214 | 
  215 |     // Exit-code chip is em-dash (D4: heuristic blocks have no reliable exit code)
  216 |     await expect(block0.getByTestId("exit-code-chip")).toHaveText("—");
  217 | 
  218 |     // --- Block 1: osc133 ---
  219 |     const block1 = blocks.nth(1);
  220 | 
  221 |     // Mode badge text is "osc133"
  222 |     await expect(block1.getByText("osc133")).toBeVisible();
  223 | 
  224 |     // Exit-code chip is "0" with green styling
  225 |     const exitChip = block1.getByTestId("exit-code-chip");
  226 |     await expect(exitChip).toHaveText("0");
  227 |     // Green styling: the chip carries border-green-600 / text-green-600
  228 |     await expect(exitChip).toHaveClass(/green/);
  229 | 
  230 |     // Command text is non-empty for the osc133 block
  231 |     // (prompt bytes "user@host:~$ " are decoded; sigil+space stripped → "user@host:~")
  232 |     const commandText = block1.getByTestId("command-text");
  233 |     await expect(commandText).not.toHaveText("—");
  234 |   });
  235 | 
  236 |   test("collapse toggle narrows the block-list sidebar", async ({ page }) => {
  237 |     // --- Auth setup ---
  238 |     await injectFakeToken(page);
  239 |     await mockUsersMe(page);
  240 | 
  241 |     // Swallow the WS connection — no bytes needed for this test.
  242 |     await page.routeWebSocket(RELAY_WS_URL, (ws) => {
  243 |       ws.onMessage(() => {});
  244 |     });
  245 | 
> 246 |     await page.goto(`/sessions/${SESSION_ID}/watch`);
      |                ^ Error: page.goto: net::ERR_CONNECTION_REFUSED at http://localhost:3000/sessions/e2e-wasm-test-session/watch
  247 | 
  248 |     const sidebar = page.getByTestId("block-list-sidebar");
  249 |     await expect(sidebar).toBeVisible({ timeout: 10_000 });
  250 | 
  251 |     // Measure initial width (open state: w-72 = 18 rem ≈ 288 px)
  252 |     const widthBefore = await sidebar.evaluate(
  253 |       (el) => (el as HTMLElement).getBoundingClientRect().width,
  254 |     );
  255 | 
  256 |     // Click the collapse toggle
  257 |     await page.getByTestId("block-list-toggle").click();
  258 | 
  259 |     // After collapse the sidebar should be significantly narrower (w-8 = 2 rem)
  260 |     await expect(async () => {
  261 |       const widthAfter = await sidebar.evaluate(
  262 |         (el) => (el as HTMLElement).getBoundingClientRect().width,
  263 |       );
  264 |       expect(widthAfter).toBeLessThan(widthBefore / 2);
  265 |     }).toPass({ timeout: 5_000 });
  266 |   });
  267 | });
  268 | 
```