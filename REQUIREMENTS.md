# ARC requirements — joint Beacon + Beam contract

> **Status:** Beacon + Beam columns now populated (beacon-claude
> 2026-05-15 + beam-claude 2026-05-14). arc-claude owns convergence
> column updates after each client's column lands.
>
> Comms thread: `/Volumes/Main External/Development/a2a-beam-beacon.jsonl`
> (file-based A2A channel; protocol in the sibling `a2a-protocol.md`).

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
| Providers | GitHub (only one wired in `arc-relay/src/api/auth_routes.rs` today); Google + Microsoft planned | **GitHub, Google, Microsoft (per ROADMAP §C4). OAuth device-flow path used by `app/src/ai/agent_loop/oauth/mod.rs`. NO anonymous-token path; Beam users without an account run offline-only against arc-relay.** | All three OAuth providers; signup is open to anyone — Beam or Beacon. **NO anonymous token path** (per user direction 2026-05-16). Cred config in arc-relay `.env`. |
| Endpoint | `/api/auth/oauth/github` (confirmed against route registration) | **Same. Beam-side wiring delegates through `crates/arc_client/src/auth.rs::ArcAuth` (currently `unimplemented!()` stub, lights up when ROADMAP §C4 lands).** | Keep existing |
| Token format | JWT HS256, 32+ char secret per `ARC_JWT_SECRET`; bearer in `Authorization` header; consumed via `beacon-net::ArcConnectionJsonRpc` (stashes in `?token=` on WS upgrade + Authorization header on REST) | **Bearer JWT. `crates/arc_client/src/auth.rs:13` defines `JwtPair { access_token, refresh_token, access_expires_at }`. Matches the Beacon expectation already.** | **JWT (HS256, 32+ char secret), bearer in `Authorization` header** |
| Session lifetime | 24h access token + 30d refresh. Refresh issued on every successful authenticated call within the last 7 days. *Spec proposal; Beacon adapts to whatever arc-relay actually wires.* | **Whatever Arc returns in `access_expires_at`. Beam refreshes proactively when `now + 60s >= access_expires_at` (cite: `crates/arc_client/src/auth.rs:50-60` planning doc-comment).** | Arc-controlled; recommended 1h access / 30d refresh |
| Refresh flow | Standard OAuth2 refresh, server-issued. Refresh token persists in macOS Keychain — same lockout policy as Copilot/MLX provider tokens; see `beacon-mac/Sources/BeaconMac/Helpers/KeychainManager.swift` | **`POST /api/auth/refresh` with `{refresh_token}`. Returns a fresh `JwtPair`. Beam persists the new pair into `AgentLoopStore` under `auth_kind = OAuth` (cite: `crates/arc_client/src/auth.rs:46-55` doc comment).** | Same on both sides |
| Surface | n/a | n/a | **HTTP routes (`POST /api/auth/{register,login,refresh}`, `GET /api/auth/oauth/:provider`), NOT JSON-RPC. Bearer JWT enforced at WS upgrade (`?token=...`) and on protected `/api/*` HTTP routes. No `arc.auth.*` JSON-RPC methods by design.** |
| Tiers | enterprise / `pro` (paid) | free (any signed-up user) | **Open signup for anyone; tier-gated features distinguish free vs `pro`. `pro` is bundled with Beacon (desktop / enterprise) users; standalone `pro` purchase via future Stripe integration. Premium features (marketplace publish per §7, audit log query per §8b, eventual A2E cross-org per §5) require `pro`. Beam OSS users get the full free tier — same auth, same wire surface, just lacks `pro`-gated features.** |

### 2. AI chat streaming

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Method | `arc.ai.stream_chat` JSON-RPC (shipped). Beacon's `beacon-ai-core::CloudProvider::stream` invokes via `beacon-net::JsonRpcWsClient`. Confirmed in production. | **Same. Beam wires it through `arc_client::ai::ArcAiClient::stream_chat` (cite: `crates/arc_client/src/ai.rs:11-28`). Used as the cloud-fallback path of `agent_loop` for users on `Backend::Arc` (cite: `app/src/settings/backend.rs`).** | Existing shipped surface |
| Request fields | model, messages, tools, system, temperature, max_tokens — confirmed shipped. **Need adding: `stop_sequences` and `response_format` (json_mode)** for tool-use pipelines; not yet wired (Beacon does pure tool_use today, structured-output is the agentic-loop fallback). | **Same. Beam's `ProviderMessage` (cite: `app/src/ai/agent_loop/loop_runner.rs`) already carries all of these. The `ChatMessage` shape in `arc_client::ai::ChatMessage` is OpenAI-compatible and round-trips through Beam's `OpenAiCompatProvider` without lossy conversion.** | Both — already aligned on shipped set; add stop_sequences + response_format per Beacon |
| Streaming format | JSON-RPC notifications carrying `chat_delta`, `tool_use_start`, `tool_use_input_delta`, `tool_use_stop`, `message_stop`. Anthropic-shape parsing in `beacon-ai-core::providers::anthropic::stream::parse_event`. Confirmed shipped + consumed. | **Same. Beam's existing SSE parser in `app/src/ai/agent_loop/providers/openai_compat.rs` reads the same `delta.content` / `tool_calls` / `usage` shape. JSON-RPC-WS framing adds an outer envelope but the inner payload matches.** | Same |
| Tools support | YES — Anthropic tool_use shape is canonical. OpenAI-compat `tool_calls` shape adapted at provider boundary (`beacon-ai-core::providers::openai_compat`). Both work in production. | **Yes. `app/src/ai/agent_loop/tool_bridge.rs` already dispatches Anthropic-style tool_use (and computer-use). No client-side changes needed.** | Same |
| Multimodal images | YES — Beacon ships native image-in-chat (Cmd+Shift+4 screenshot → drag into AI panel) and agent-side via `beacon-computer-use::ComputerUseDispatcher` (screenshots through `screencapture`). Need an Arc-level image envelope that doesn't re-encode. **Adopt beam-001's `MessageImage` shape (PR #111) as canonical.** | YES — Beam shipped `MessageImage` and routed through ComputerUseDispatcher in PR #111 | Define a common image envelope; Beam's MessageImage is the reference |

### 3. Session mirroring + remote control

> Beam is a **receive-only consumer** here. The "Send to Cloud" affordance (ROADMAP §C1) hands a local conversation off to a cloud agent; Beam then attaches+subscribes to mirror its output back into the local transcript. Beam does not host inbound sessions.

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Attach | `arc.session.attach` (shipped) — consumed at `beacon-mac/Sources/BeaconMac/Controllers/ArcSessionController.swift::attach` | **Beam attaches when the user hits "Send to Cloud" (ROADMAP §C1). The cloud agent's `session_id` is returned by `arc.cloud_agents.spawn` (cite: `crates/arc_client/src/cloud_agents.rs:18-26`) and immediately attached for live-mirror.** | Existing shipped surface |
| Subscribe | `arc.session.subscribe` (shipped) — same controller, post-attach | **Same. Beam subscribes to push-output + block-lifecycle notifications and feeds them into the local conversation view.** | Same |
| Control grant flow | Approve/deny on inbound `control_requested` via `BeaconMainWindowController` modal; 30s grant timeout enforced client-side via `Timer.scheduledTimer` | **Beam users may approve OUTBOUND control grants (sharing their local session to a cloud-attached observer) but won't typically receive grant requests in v0. Approve/deny UI lives in the existing `Settings → Sharing` page; default policy: prompt on every request.** | Same wire; per-client UI |
| Frame format | Base64-encoded VT bytes (shipped); **also need `arc.mirror.resize` notif** for pty resize events. Beacon emits resize on Cmd+Plus/Cmd+Minus + window resize. | **Same. Beam's terminal model (`crates/vt100`) consumes raw VT bytes; base64 decode is trivial.** | base64-encoded VT bytes + resize notif |
| Block replay | `replay_begin/block/end` (shipped); arc-web consumes via WASM block detector. **Gap on Beacon Mac side: doesn't yet consume replay on attach to a session it didn't create.** Beacon-claude tracking. | **Beam consumes these on attach to repopulate scrollback after reconnect — necessary because the local conversation transcript needs to be coherent even if Beam was offline while the cloud agent ran.** | Same |
| Cross-org attach | **Enterprise default-deny.** Per-user-can-attach allowlist persists in beacon-mac `UserDefaults`. **Audit-log event regardless** of accept/deny — enterprise SOC2 requirement. | Beam: open with block-list | Per-host policy; enterprise toggle |

### 4. User-to-user messaging (`arc.msg.*` — proposed, not shipped)

> **Beam scope:** not in v0. Beam OSS user base isn't org-scoped and has no concrete call site for DMs today. Could matter for community-plugin-author DM features post-v1; deferred for now.

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Within-org DM | YES — primary use case for enterprise teams. Default-allow for org-mates. Beacon emits/consumes via dedicated `BeaconMessagingController` (not yet implemented; spec'd in P3e follow-ups). | **n/a (no orgs in Beam v0)** | Org-scoped on Beacon, open on Beam |
| Cross-org DM | **Enterprise-admin-gated, off by default.** Federated workspaces (vendor ↔ customer org) need this; admin enables per-other-org. | **Defer to v1; Beam's user base isn't org-scoped today** | (TBD; revisit when Beam has at least one concrete call site) |
| Channels | YES for v1 — per-team broadcast channels for status updates (deploys, alerts). Optional `#general` per workspace. | **Defer to v1** | (TBD) |
| Session attach as message attachment | **YES — major use case.** "Look at this" is the killer feature: send a message containing a `session_ref`; recipient clicks → their client opens attach + subscribe to the live session. **Wire shape proposal: `{kind: "session_ref", session_id, attach_token, expires_at}`.** | **Interesting future affordance once Beam has user-discoverability; defer to v1** | Adopt Beacon's proposal |
| Block ref as message attachment | YES — share a single command's output (snapshot, not live). **Wire shape: `{kind: "block_ref", session_id, block_id}`.** | (TBD) | Adopt Beacon's proposal |
| File attachment | YES for screenshots + small text. URL-based via the same R2 bucket as marketplace. | (TBD) | Same |
| Read receipts | Opt-out per-thread. **Enterprise admin can force-on for compliance.** | (TBD) | Per-thread default + enterprise override |
| Rate limit on first-DM | Low for org-mates (10/min); tight for cross-org (3/hr) — anti-spam from peer orgs. | **N/A in v0; would be tight (anti-spam) if Beam ever enables DMs** | per-client policy |

### 5. Agent-to-Execution (`arc.exec.*` — proposed, not shipped)

> **Beam scope:** Beam is a TARGET-self consumer in v0 (Beam's local agent can spawn shell/exec on the user's own machine via `bb-tool-bridge`-style infrastructure already in `app/src/ai/agent_loop/tool_bridge.rs`). Beam-as-arbitrary-target (other users, plugins) defers to v1.

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Target self | YES — already exists via `beacon-hostd` host registration. Beacon's local agent loop uses `RealExecutor` (`beacon-ffi/src/lib.rs::RealExecutor::dispatch`). Migrating to `arc.exec.spawn { target: Self }` is mostly renaming the dispatch boundary. | **yes — Beam already spawns shell commands locally via `tool_bridge`; can adopt `arc.exec.spawn { target: Self }` as a wire-protocol unification (beam-001 §B ack)** | unified `arc.exec.spawn { target: Self }` — **typed enum, not free-form string** (beam-001 §B) |
| Target another user | YES, within-org, with consent prompt. Use case: "Alice's agent asks Bob's machine to deploy because Bob has the staging creds in Keychain." Consent UI = same modal as remote-control grant. | **Defer to v1. cross-org, with consent** | same wire; differ on org gate |
| Target plugin executor | YES — `beacon-plugin-host` (Slice C) is the receiver. **Kubernetes is the relevant canonical** for our enterprise customers, not Slurm. **Concur with beam-001 §F's K8s preference.** | **yes (beam-001 §F: K8s as canonical example over Slurm). Local plugin executors first (i.e., target the user's own K8s context); cloud-plugin-executors defer to v1.** | same; **Kubernetes as the reference plugin** |
| Sandbox / CommandPolicy | **Beacon owns `beacon-command`** (CommandPolicy + DangerGate). Lives in `WarpClone/beacon-core/crates/beacon-command/`. Beam path-deps it per beam-001 §D. If usage stabilizes, this crate could migrate to `ARC/crates/` — not urgent. | **Beam adopts via path-dep** (beam-001 §D) | `beacon-command` is the shared sandbox layer; Beacon owns evolution. Crate may need to move into `ARC/crates/` when Beam path-deps it. **Resolved 2026-05-14.** |
| Audit log | **REQUIRED** for enterprise SOC2/auditing. Every `arc.exec.requested` + `arc.exec.granted` + `arc.exec.exit` must persist to a durable log queryable by an admin role. Schema in Appendix A. Retention: 90d hot, 7y cold. | **Opt-in. Beam's default telemetry posture is off (per ROADMAP §C6 `/api/v1/telemetry`); audit log inherits that. Power-users / enterprise users who opt in get the same `arc.exec.requested` event stream Beacon gets.** | yes; emit `arc.exec.requested` event; per-user opt-in for Beam, default-on for Beacon |

### 6. Plugin host (gRPC contracts)

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Runtime | Beacon's `beacon-plugin-host` (in WarpClone, Slice C). Beacon owns evolution. | **Adopt via path-dep** (beam-001 §G) | Same crate; Beacon owns evolution |
| Bundle format | `.beaconplug` (zip + Ed25519 signature). | **Same wire format; signature verification is OPTIONAL on Beam (warn on unsigned + allow install; Beacon refuses unsigned). Install path: `~/.beam/skills/<plugin-slug>/` (cite: ROADMAP §C5).** | Same |
| Capability traits | Executor, Messenger, TerminalProvider, DataSource, EventSink — **the 5-set is sufficient.** Add only if a real plugin can't be expressed. | **Ack 5-set sufficient for v0** (beam-001 §A); optional `Notifier`/`Alarm` deferred as composable from `EventSink + Executor` | **Ship the 5 as-is.** Add only if a real plugin can't be expressed. |
| Federation | Beacon: org-shareable in v1. | Beam: public-allowlisted | **v0: inert scaffolding** (`arc.plugin.discover` + per-plugin ACL hooks defined but no implementation). **v1: federation lights up.** (beam-001 §C) **Resolved 2026-05-14.** |

### 7. Marketplace (browse, install, publish)

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Browse | YES — **signed-by-default for enterprise.** UI enforces signature presence before showing the install button. Unsigned plugins surface a "developer mode" badge and require admin override per-install. | **yes (unsigned-OK, same UI). Beam consumes the read-only `GET /api/marketplace/*` browse routes via `arc_client::marketplace::MarketplacePlugin` (cite: `crates/arc_client/src/marketplace.rs:17-26`).** | `beam.ailuxis.com` serves both |
| Install | YES — signature verification via Ed25519 (beacon-cli signing pipeline). Failed verification = hard fail in production builds, no override path. | **yes, warn-on-unsigned. Install drops into `~/.beam/skills/<plugin-slug>/` (NOT `~/.beacon/...`). Per ROADMAP §C5: warn-on-unsigned, no signature requirement.** | Same routes; per-client policy on verification strictness |
| Publish | YES — paid feature for `pro` org tier. Free orgs can install but not publish. | **NO (read-only). Beam users see no "Submit Plugin" affordance; the surface is publish-only for `pro` role accounts on Beacon.** | Publish requires `pro` role |
| Org allowlist | YES — enterprise admin curates which plugins their org can install. Stored in `org_plugin_allowlist` table on arc-relay; Beacon enforces client-side by hiding non-allowed plugins from the marketplace UI. | **n/a (Beam has no orgs)** | Beacon-only feature |

### 8. New endpoints needed (per-client)

#### 8a. Beam-required endpoints (from WarpMod ROADMAP §C6)

|  | Status | Owner | Notes |
|---|---|---|---|
| `/api/current_time` | Not shipped | arc-claude | trivial — clock-skew checks; Beam's `agent_loop` calls it pre-streaming to align timestamps |
| `/api/v1/telemetry` | Not shipped | arc-claude | opt-in only, off by default for Beam OSS |
| `/api/v1/conversations/sync` | Not shipped | arc-claude (Beam-spec) | push/pull `agent_loop_conversations` between local SqliteStore and Arc — needs schema design between beam-claude + arc-claude; Beam's existing local conversation schema lives in `app/src/ai/agent_loop/store/` |
| `/api/v1/voice/transcribe` | Not shipped | arc-claude | server-side Whisper; expensive — gate behind plan; replaces hosted Warp `/ai/transcribe` (cite: ROADMAP §C "What Beam currently calls into hosted Warp for") |
| `/api/v1/relevant_files` | Not shipped | arc-claude (low priority) | local heuristic works for v0; Beam may never need the Arc-hosted version |
| `/api/v1/credential_meter` | Not shipped | arc-claude (post-billing) | only matters when Arc bills credits |

#### 8b. Beacon-required endpoints (enterprise / paying-customer surface)

These are NOT in Beam's needs because Beam is OSS / consumer-mode; they exist for Beacon's enterprise sales motion.

|  | Status | Owner | Notes |
|---|---|---|---|
| `POST /api/v1/orgs` + admin endpoints (CRUD members, invite, role) | Not shipped | arc-claude | Enterprise org provisioning + member management. Foundation of multi-tenant Beacon. |
| `GET /api/v1/orgs/:id/audit-log` | Not shipped | arc-claude | SOC2 audit trail of exec + msg + plugin events. Admin-role only. Schema = Appendix A. |
| `POST /api/v1/billing/seats` + Stripe webhooks | Not shipped | arc-claude (defer until Stripe wired) | Per-seat enterprise billing. Per-org subscription with seat allocation. |
| `arc.attest.*` JSON-RPC (proposed) | Not shipped | post-v1 | Trusted-device attestation for high-security customers (Beacon's MDM customers gate Arc access on device posture). |

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


## Shipped wire surface (authoritative)

The complete enumeration of every JSON-RPC method and notification the
relay actually serves lives in **`arc-relay/docs/rpc-method-inventory.md`**.
Current counts: 22 methods + 22 notifications = 44 total, concentrated in
§2 (AI: 7), §3 (mirroring: 24), §8 (host protocol: 13). §4 and §5 are
proposed-only; zero handlers exist for `arc.msg.*` or `arc.exec.*` today.

The inventory file is regenerated from source; treat it as the source of
truth when wires diverge from this doc.

## Appendix A — Audit log schema for `arc.exec.*`

Per Beacon's SOC2 requirement. Every state transition of an exec emits one
row to the durable audit log.

```rust
struct AuditLogEntry {
    ts:            DateTime<Utc>,            // ISO-8601 UTC
    event:         AuditEventKind,           // see enum below
    actor_user_id: String,                   // uuid; who initiated
    actor_org_id:  Option<String>,           // null for personal (free) users
    target:        AuditTarget,              // { kind, id }
    exec_id:       String,                   // ulid, joinable across events
    command:       AuditCommand,             // { kind, summary } — summary truncated to 4KB
    capabilities:  Vec<String>,              // e.g. ["fs.read", "net.out", "subprocess.spawn"]
    decision:      Option<AuditDecision>,    // populated on .requested+; null at .exit
    exit_status:   Option<i32>,              // only on .exit
    duration_ms:   Option<i64>,              // only on .exit
    client_meta:   AuditClientMeta,          // { app, version, host_id }
}

enum AuditEventKind {
    ExecRequested,
    ExecGranted,
    ExecDenied,
    ExecExit,
    ExecCancelled,
}

enum AuditDecision { Allowed, Denied, Timeout }

struct AuditTarget {
    kind: TargetKind,   // Self | User | Plugin
    id:   String,       // host_id | user_id@org_id | plugin_id/executor_id
}

struct AuditCommand {
    kind:    CommandKind,  // Shell | Tool | Structured
    summary: String,       // free-form, truncated to 4KB
}

struct AuditClientMeta {
    app:     ClientApp,    // Beacon | Beam
    version: String,
    host_id: String,
}
```

**Storage:**

- Hot: Postgres table `exec_audit_log` with index on `(org_id, ts DESC)` for admin query path. 90d retention.
- Cold: compressed JSONL (one file per day) shipped to R2 after 90d. 7-year retention for SOC2 compliance.

**Query API:**

- `GET /api/v1/orgs/:org_id/audit-log?from=<ts>&to=<ts>&user_id=&event=&decision=`
  → Admin-role only; bearer JWT must carry `org_role: admin` claim.
- Pagination via `?cursor=` + opaque cursor tokens. Default page size 100, max 1000.
- Cold-tier queries (>90d old) automatically rehydrate from R2 — slower (5-30s) but transparent to caller.

---

**Edit log**

| Date | By | Change |
|---|---|---|
| 2026-05-13 | arc-claude (extraction session) | Initial stub. |
| 2026-05-14 | arc-claude | beam-001 resolutions integrated: §5 typed-enum target + K8s reference + sandbox via path-dep; §6 5-trait ack + federation deferred to v1; §1 convergence row clarifying auth is HTTP not JSON-RPC. Q3 and Q4 resolved. Linked to `arc-relay/docs/rpc-method-inventory.md` for shipped-surface ground truth. |

