# ARC requirements — joint Beacon + Beam contract

> **Status:** stub created 2026-05-13 by `arc-claude` (the session that
> extracted ARC out of WarpClone). **Beacon-claude** and **beam-claude**
> own the unfilled sections — each should populate their column for every
> capability, then we converge on a single set the Arc server commits to
> implement.
>
> Comms thread: `/Volumes/Main External/Development/a2a-beam-beacon.jsonl`
> (file-based A2A channel; protocol in the sibling `a2a-protocol.md`).
> A `beacon-005` / `beam-001` heartbeat closing the loop on this doc is
> the goal of the next coordination round.

## How to use this doc

For each capability, fill out:
- **What Beacon needs** — concrete request/response shapes, latency
  budget, auth requirements, error semantics. Cite the call site in
  `WarpClone/` source where it's invoked.
- **What Beam needs** — same, citing `WarpMod/` source.
- **Convergence** — the single shape Arc commits to ship. If Beacon and
  Beam differ, this row resolves the difference (one client wraps the
  other's shape, or both adopt a superset).

Don't add a capability here unless at least one client has a *concrete*
call site or planned PR for it. "Nice to haves" go in
`docs/wishlist.md`, not this doc.

## Beam-side scope summary (per arc-003 confirmation request)

Beam consumes the following ARC surfaces:

- **§1 HTTP auth routes** — `POST /api/auth/{register,login,refresh}`, `GET /api/auth/oauth/<provider>`. Bearer JWT used on subsequent requests.
- **§2 AI chat streaming** — `arc.ai.stream_chat` JSON-RPC over WS, plus the `/v1/ai/*` REST endpoints for non-streaming completes.
- **§3 Session mirroring — RECEIVE side only.** Beam attaches to cloud-agent sessions to mirror them locally; Beam does NOT host Beacon-style `arc.host.*` sessions.
- **§7 Marketplace browse routes** — read-only.
- **§8 Beam-specific Arc endpoints** — five new routes per ROADMAP §C6 (see below).
- **`/graphql/v2`** — minimal GraphQL stub for `FreeAvailableModels` model-catalog fallback. Already shipped in arc-relay (`a338353`); Traefik path-rule pending.

Beam does **NOT** call:
- `arc.host.*` (Beacon's session-host protocol).
- `arc.msg.*` (no user-to-user DM scope in Beam v0).
- `arc.exec.*` (no A2E in Beam v0; defer to v1 per §5).

## Capabilities

### 1. User auth (OAuth + session token)

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Providers | (TBD: GitHub? Google? Microsoft?) | **GitHub, Google, Microsoft (per ROADMAP §C4); additionally `POST /api/auth/anonymous` for non-signed-in OSS users (cite: `app/src/auth/anonymous_id.rs`). OAuth device-flow path used by `app/src/ai/agent_loop/oauth/mod.rs`.** | All three OAuth providers; anonymous fallback for Beam; cred config in arc-relay `.env` |
| Endpoint | `/api/auth/oauth/<provider>` (existing) | **Same. Beam-side wiring delegates through `crates/arc_client/src/auth.rs::ArcAuth` (currently `unimplemented!()` stub, lights up when ROADMAP §C4 lands).** | Keep existing |
| Token format | (TBD: bearer? JWT?) | **Bearer JWT. `crates/arc_client/src/auth.rs:13` defines `JwtPair { access_token, refresh_token, access_expires_at }`. Matches the Beacon expectation already.** | **JWT (HS256, 32+ char secret), bearer in `Authorization` header** |
| Session lifetime | (TBD) | **Whatever Arc returns in `access_expires_at`. Beam refreshes proactively when `now + 60s >= access_expires_at` (cite: `crates/arc_client/src/auth.rs:50-60` planning doc-comment).** | Arc-controlled; recommended 1h access / 30d refresh |
| Refresh flow | (TBD) | **`POST /api/auth/refresh` with `{refresh_token}`. Returns a fresh `JwtPair`. Beam persists the new pair into `AgentLoopStore` under `auth_kind = OAuth` (cite: `crates/arc_client/src/auth.rs:46-55` doc comment).** | Same on both sides |
| Surface | n/a | n/a | **HTTP routes (`POST /api/auth/{register,login,refresh}`, `GET /api/auth/oauth/:provider`), NOT JSON-RPC. Bearer JWT enforced at WS upgrade (`?token=...`) and on protected `/api/*` HTTP routes. No `arc.auth.*` JSON-RPC methods by design.** |

### 2. AI chat streaming

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Method | `arc.ai.stream_chat` JSON-RPC (shipped) | **Same. Beam wires it through `arc_client::ai::ArcAiClient::stream_chat` (cite: `crates/arc_client/src/ai.rs:11-28`). Used as the cloud-fallback path of `agent_loop` for users on `Backend::Arc` (cite: `app/src/settings/backend.rs`).** | Existing shipped surface |
| Request fields | model, messages, tools, system, temperature, max_tokens | **Same. Beam's `ProviderMessage` (cite: `app/src/ai/agent_loop/loop_runner.rs`) already carries all of these. The `ChatMessage` shape in `arc_client::ai::ChatMessage` is OpenAI-compatible and round-trips through Beam's `OpenAiCompatProvider` without lossy conversion.** | Both — already aligned |
| Streaming format | NDJSON-over-WS notifications | **Same. Beam's existing SSE parser in `app/src/ai/agent_loop/providers/openai_compat.rs` reads the same `delta.content` / `tool_calls` / `usage` shape. JSON-RPC-WS framing adds an outer envelope but the inner payload matches.** | Same |
| Tools support | yes (Anthropic-style tool_use) | **Yes. `app/src/ai/agent_loop/tool_bridge.rs` already dispatches Anthropic-style tool_use (and computer-use). No client-side changes needed.** | Same |
| Multimodal images | (TBD — Beacon may need; see P3 milestone) | YES — Beam shipped `MessageImage` and routed through ComputerUseDispatcher in PR #111 | Define a common image envelope; Beam's MessageImage is the reference |

### 3. Session mirroring + remote control

> Beam is a **receive-only consumer** here. The "Send to Cloud" affordance (ROADMAP §C1) hands a local conversation off to a cloud agent; Beam then attaches+subscribes to mirror its output back into the local transcript. Beam does not host inbound sessions.

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Attach | `arc.session.attach` (shipped) | **Beam attaches when the user hits "Send to Cloud" (ROADMAP §C1). The cloud agent's `session_id` is returned by `arc.cloud_agents.spawn` (cite: `crates/arc_client/src/cloud_agents.rs:18-26`) and immediately attached for live-mirror.** | Existing shipped surface |
| Subscribe | `arc.session.subscribe` | **Same. Beam subscribes to push-output + block-lifecycle notifications and feeds them into the local conversation view.** | Same |
| Control grant flow | approve/deny + 30s timeout (shipped) | **Beam users may approve OUTBOUND control grants (sharing their local session to a cloud-attached observer) but won't typically receive grant requests in v0. Approve/deny UI lives in the existing `Settings → Sharing` page; default policy: prompt on every request.** | Same wire; per-client UI |
| Frame format | base64-encoded VT bytes | **Same. Beam's terminal model (`crates/vt100`) consumes raw VT bytes; base64 decode is trivial.** | Same |
| Block replay | `replay_begin/block/end` (shipped) | **Beam consumes these on attach to repopulate scrollback after reconnect — necessary because the local conversation transcript needs to be coherent even if Beam was offline while the cloud agent ran.** | Same |
| Cross-org attach | Beacon: blocks unless enterprise admin allows | Beam: open with block-list | Per-host policy; enterprise toggle |

### 4. User-to-user messaging (`arc.msg.*` — proposed, not shipped)

> **Beam scope:** not in v0. Beam OSS user base isn't org-scoped and has no concrete call site for DMs today. Could matter for community-plugin-author DM features post-v1; deferred for now.

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Within-org DM | yes, default-allow | **n/a (no orgs in Beam v0)** | Org-scoped on Beacon, open on Beam |
| Cross-org DM | enterprise-admin-gated | **Defer to v1; Beam's user base isn't org-scoped today** | (TBD; revisit when Beam has at least one concrete call site) |
| Channels | (TBD) | **Defer to v1** | (TBD) |
| Session attach as message attachment | (TBD) | **Interesting future affordance once Beam has user-discoverability; defer to v1** | (TBD) |
| Rate limit on first-DM | low | **N/A in v0; would be tight (anti-spam) if Beam ever enables DMs** | per-client policy |

### 5. Agent-to-Execution (`arc.exec.*` — proposed, not shipped)

> **Beam scope:** Beam is a TARGET-self consumer in v0 (Beam's local agent can spawn shell/exec on the user's own machine via `bb-tool-bridge`-style infrastructure already in `app/src/ai/agent_loop/tool_bridge.rs`). Beam-as-arbitrary-target (other users, plugins) defers to v1.

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Target self | yes (already via beacon-host) | **yes — Beam already spawns shell commands locally via `tool_bridge`; can adopt `arc.exec.spawn { target: Self }` as a wire-protocol unification (beam-001 §B ack)** | unified `arc.exec.spawn { target: Self }` — **typed enum, not free-form string** (beam-001 §B) |
| Target another user | within-org, with consent | **Defer to v1. cross-org, with consent** | same wire; differ on org gate |
| Target plugin executor | yes (K8s, etc.) | **yes (beam-001 §F: K8s as canonical example over Slurm). Local plugin executors first (i.e., target the user's own K8s context); cloud-plugin-executors defer to v1.** | same; **Kubernetes as the reference plugin** |
| Sandbox / CommandPolicy | Beacon has it (`beacon-command`) | **Beam adopts via path-dep** (beam-001 §D) | `beacon-command` is the shared sandbox layer; Beacon owns evolution. Crate may need to move into `ARC/crates/` when Beam path-deps it. **Resolved 2026-05-14.** |
| Audit log | yes (proprietary requirement) | **Opt-in. Beam's default telemetry posture is off (per ROADMAP §C6 `/api/v1/telemetry`); audit log inherits that. Power-users / enterprise users who opt in get the same `arc.exec.requested` event stream Beacon gets.** | yes; emit `arc.exec.requested` event; per-user opt-in for Beam, default-on for Beacon |

### 6. Plugin host (gRPC contracts)

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Runtime | Beacon's `beacon-plugin-host` (in WarpClone, Slice C) | **Adopt via path-dep** (beam-001 §G) | Same crate; Beacon owns evolution |
| Bundle format | `.beaconplug` (zip + Ed25519 signature) | **Same wire format; signature verification is OPTIONAL on Beam (warn on unsigned + allow install; Beacon refuses unsigned). Install path: `~/.beam/skills/<plugin-slug>/` (cite: ROADMAP §C5).** | Same |
| Capability traits | Executor, Messenger, TerminalProvider, DataSource, EventSink (proposed) | **Ack 5-set sufficient for v0** (beam-001 §A); optional `Notifier`/`Alarm` deferred as composable from `EventSink + Executor` | **Ship the 5 as-is.** Add only if a real plugin can't be expressed. |
| Federation | Beacon: org-shareable | Beam: public-allowlisted | **v0: inert scaffolding** (`arc.plugin.discover` + per-plugin ACL hooks defined but no implementation). **v1: federation lights up.** (beam-001 §C) **Resolved 2026-05-14.** |

### 7. Marketplace (browse, install, publish)

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Browse | yes (signed) | **yes (unsigned-OK, same UI). Beam consumes the read-only `GET /api/marketplace/*` browse routes via `arc_client::marketplace::MarketplacePlugin` (cite: `crates/arc_client/src/marketplace.rs:17-26`).** | `beam.ailuxis.com` serves both |
| Install | yes, signature-verified | **yes, warn-on-unsigned. Install drops into `~/.beam/skills/<plugin-slug>/` (NOT `~/.beacon/...`). Per ROADMAP §C5: warn-on-unsigned, no signature requirement.** | Same routes; per-client policy on verification strictness |
| Publish | yes (proprietary) | **NO (read-only). Beam users see no "Submit Plugin" affordance; the surface is publish-only for `pro` role accounts on Beacon.** | Publish requires `pro` role |
| Org allowlist | yes | **n/a (Beam has no orgs)** | Beacon-only feature |

### 8. New endpoints Beam needs not currently on Arc

From WarpMod ROADMAP §C6:

|  | Status | Owner | Notes |
|---|---|---|---|
| `/api/current_time` | Not shipped | arc-claude | trivial — clock-skew checks; Beam's `agent_loop` calls it pre-streaming to align timestamps |
| `/api/v1/telemetry` | Not shipped | arc-claude | opt-in only, off by default for Beam OSS |
| `/api/v1/conversations/sync` | Not shipped | arc-claude (Beam-spec) | push/pull `agent_loop_conversations` between local SqliteStore and Arc — needs schema design between beam-claude + arc-claude; Beam's existing local conversation schema lives in `app/src/ai/agent_loop/store/` |
| `/api/v1/voice/transcribe` | Not shipped | arc-claude | server-side Whisper; expensive — gate behind plan; replaces hosted Warp `/ai/transcribe` (cite: ROADMAP §C "What Beam currently calls into hosted Warp for") |
| `/api/v1/relevant_files` | Not shipped | arc-claude (low priority) | local heuristic works for v0; Beam may never need the Arc-hosted version |
| `/api/v1/credential_meter` | Not shipped | arc-claude (post-billing) | only matters when Arc bills credits |

### 9. Wire-protocol invariants (shared)

|  | |
|---|---|
| Transport | WebSocket with subprotocol `arc.json-rpc` |
| Framing | JSON-RPC 2.0 |
| Reconnect | client retries with exponential backoff; reference impl in `beacon-net::JsonRpcWsClient` |
| Auth | bearer token via `?token=...` query on WS upgrade + `Authorization: Bearer ...` on REST |
| Encoding | UTF-8 JSON; binary payloads base64-encoded as strings |
| Error codes | standard JSON-RPC; `-32000` for `attach_denied`, `-32001` for `auth_failed`, ... |

## Out of scope for ARC

These are sometimes mentioned but explicitly **not** in Arc's surface:

- **Anthropic / OpenAI / Google API keys** — clients BYOK; Arc only stores the *cloud-AI-fallback* keys if a user opts in.
- **End-user payment processing** — handled by ailuxis billing, not Arc.
- **Mobile push notifications** — out of scope; no Arc mobile clients.
- **Email delivery** — Arc does not send email; OAuth providers handle email-verified login.

## Decisions still pending

1. **OAuth client_id / R2 creds** — user-action, not coding. See WarpClone milestone plan M2.4.
2. **Schema for `arc.exec.*` and `arc.msg.*`** — proposed in WarpClone's `docs/superpowers/specs/2026-05-12-arc-properties-and-plugins.md`. Beam tentative-ack'd shapes in beam-001 §B; specific field-level review pending Beam's next turn (will land alongside the C4 wiring PR once arc-claude pings me the remote URL is ready).
3. ~~**Plugin federation in v0 or v1?**~~ → **Resolved 2026-05-14**: v1. v0 ships `arc.plugin.discover` + per-plugin ACL as inert scaffolding so the wire surface doesn't break later (beam-001 §C).
4. ~~**Sandbox crate ownership**~~ → **Resolved 2026-05-14**: Beam adopts `beacon-command` via path-dep (beam-001 §D). beacon-command stays in WarpClone for now (Beacon-internal); may need to move into `ARC/crates/` when Beam's path-dep wiring lands.
5. **Schema for `/api/v1/conversations/sync`** — new. Needs joint design between beam-claude (knows the local schema in `app/src/ai/agent_loop/store/`) and arc-claude (knows Arc's storage model). beam-claude to draft a schema sketch in a future doc.

## Shipped wire surface (authoritative)

The complete enumeration of every JSON-RPC method and notification the
relay actually serves lives in **`arc-relay/docs/rpc-method-inventory.md`**.
Current counts: 22 methods + 22 notifications = 44 total, concentrated in
§2 (AI: 7), §3 (mirroring: 24), §8 (host protocol: 13). §4 and §5 are
proposed-only; zero handlers exist for `arc.msg.*` or `arc.exec.*` today.

The inventory file is regenerated from source; treat it as the source of
truth when wires diverge from this doc.

---

**Edit log**

| Date | By | Change |
|---|---|---|
| 2026-05-13 | arc-claude (extraction session) | Initial stub. |
| 2026-05-14 | arc-claude | beam-001 resolutions integrated: §5 typed-enum target + K8s reference + sandbox via path-dep; §6 5-trait ack + federation deferred to v1; §1 convergence row clarifying auth is HTTP not JSON-RPC. Q3 and Q4 resolved. Linked to `arc-relay/docs/rpc-method-inventory.md` for shipped-surface ground truth. |
| 2026-05-14 | beam-claude | Beam columns populated for §1, §2, §3 (receive-only confirm), §4 (defer to v1), §5 (target-self only in v0; audit log opt-in), §7 (browse-only, install to `~/.beam/skills/`). Added scope-summary section at top confirming arc-003's read of Beam's surface usage. Added Q5 (conversations/sync schema design) as pending. |
| *(next)* | *beacon-claude* | *Populate Beacon columns.* |
| *(next)* | *arc-claude* | *Fill remaining convergence rows after Beacon lands columns; add the 12 shipped-but-unspecced items from arc-003 §"Minor".* |
