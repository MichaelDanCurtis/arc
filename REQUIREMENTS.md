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

## Capabilities

### 1. User auth (OAuth + session token)

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Providers | (TBD: GitHub? Google? Microsoft?) | (TBD) | All three; cred config in arc-relay `.env` |
| Endpoint | `/api/auth/oauth/<provider>` (existing) | (TBD) | Keep existing |
| Token format | (TBD: bearer? JWT?) | (TBD) | **JWT (HS256, 32+ char secret), bearer in `Authorization` header** |
| Session lifetime | (TBD) | (TBD) | (TBD) |
| Refresh flow | (TBD) | (TBD) | (TBD) |
| Surface | n/a | n/a | **HTTP routes (`POST /api/auth/{register,login,refresh}`, `GET /api/auth/oauth/:provider`), NOT JSON-RPC. Bearer JWT enforced at WS upgrade (`?token=...`) and on protected `/api/*` HTTP routes. No `arc.auth.*` JSON-RPC methods by design.** |

### 2. AI chat streaming

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Method | `arc.ai.stream_chat` JSON-RPC (shipped) | Beam's `agent_loop` may also call this for cloud-fallback (TBD) | Existing shipped surface |
| Request fields | model, messages, tools, system, temperature, max_tokens | (TBD) | Both — already aligned |
| Streaming format | NDJSON-over-WS notifications | (TBD) | Same |
| Tools support | yes (Anthropic-style tool_use) | (TBD) | Same |
| Multimodal images | (TBD — Beacon may need; see P3 milestone) | YES — Beam shipped `MessageImage` and routed through ComputerUseDispatcher in PR #111 | Define a common image envelope; Beam's MessageImage is the reference |

### 3. Session mirroring + remote control

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Attach | `arc.session.attach` (shipped) | (TBD — Beam's session-share roadmap) | Existing shipped surface |
| Subscribe | `arc.session.subscribe` | (TBD) | Same |
| Control grant flow | approve/deny + 30s timeout (shipped) | (TBD) | Same |
| Frame format | base64-encoded VT bytes | (TBD) | Same |
| Block replay | `replay_begin/block/end` (shipped) | (TBD) | Same |
| Cross-org attach | Beacon: blocks unless enterprise admin allows | Beam: open with block-list | Per-host policy; enterprise toggle |

### 4. User-to-user messaging (`arc.msg.*` — proposed, not shipped)

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Within-org DM | yes, default-allow | n/a (no orgs in Beam) | Org-scoped on Beacon, open on Beam |
| Cross-org DM | enterprise-admin-gated | yes | (TBD) |
| Channels | (TBD) | (TBD) | (TBD) |
| Session attach as message attachment | (TBD) | (TBD) | (TBD) |
| Rate limit on first-DM | low | tight (anti-spam) | per-client policy |

### 5. Agent-to-Execution (`arc.exec.*` — proposed, not shipped)

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Target self | yes (already via beacon-host) | yes (beam-001 §B ack) | unified `arc.exec.spawn { target: Self }` — **typed enum, not free-form string** (beam-001 §B) |
| Target another user | within-org, with consent | cross-org, with consent | same wire; differ on org gate |
| Target plugin executor | yes (K8s, etc.) | yes (beam-001 §F: prefer K8s as canonical example over Slurm) | same; **Kubernetes as the reference plugin** |
| Sandbox / CommandPolicy | Beacon has it (`beacon-command`) | **Beam adopts via path-dep** (beam-001 §D) | `beacon-command` is the shared sandbox layer; Beacon owns evolution. Crate may need to move into `ARC/crates/` when Beam path-deps it. **Resolved 2026-05-14.** |
| Audit log | yes (proprietary requirement) | (TBD) | yes; emit `arc.exec.requested` event |

### 6. Plugin host (gRPC contracts)

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Runtime | Beacon's `beacon-plugin-host` (in WarpClone, Slice C) | **Adopt via path-dep** (beam-001 §G) | Same crate; Beacon owns evolution |
| Bundle format | `.beaconplug` (zip + Ed25519 signature) | Same | Same |
| Capability traits | Executor, Messenger, TerminalProvider, DataSource, EventSink (proposed) | **Ack 5-set sufficient for v0** (beam-001 §A); optional `Notifier`/`Alarm` deferred as composable from `EventSink + Executor` | **Ship the 5 as-is.** Add only if a real plugin can't be expressed. |
| Federation | Beacon: org-shareable | Beam: public-allowlisted | **v0: inert scaffolding** (`arc.plugin.discover` + per-plugin ACL hooks defined but no implementation). **v1: federation lights up.** (beam-001 §C) **Resolved 2026-05-14.** |

### 7. Marketplace (browse, install, publish)

|  | Beacon | Beam | Convergence |
|---|---|---|---|
| Browse | yes (signed) | yes (unsigned-OK, same UI) | `beam.ailuxis.com` serves both |
| Install | yes, signature-verified | yes, warn on unsigned | Same routes; per-client policy on verification strictness |
| Publish | yes (proprietary) | NO (read-only) | Publish requires `pro` role |
| Org allowlist | yes | n/a | Beacon-only feature |

### 8. New endpoints Beam needs not currently on Arc

From WarpMod ROADMAP §C6:

|  | Status | Owner | Notes |
|---|---|---|---|
| `/api/current_time` | Not shipped | arc-claude | trivial — clock-skew checks |
| `/api/v1/telemetry` | Not shipped | arc-claude | opt-in only, off by default |
| `/api/v1/conversations/sync` | Not shipped | arc-claude (Beam-spec) | needs schema design with beam-claude |
| `/api/v1/voice/transcribe` | Not shipped | arc-claude | server-side Whisper; expensive — gate behind plan |
| `/api/v1/relevant_files` | Not shipped | arc-claude (low priority) | local heuristic works for v0 |
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
2. **Schema for `arc.exec.*` and `arc.msg.*`** — proposed in WarpClone's `docs/superpowers/specs/2026-05-12-arc-properties-and-plugins.md`. Beam tentative-ack'd shapes in beam-001 §B; specific field-level review pending Beam's next turn.
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

---

**Edit log**

| Date | By | Change |
|---|---|---|
| 2026-05-13 | arc-claude (extraction session) | Initial stub. |
| 2026-05-14 | arc-claude | beam-001 resolutions integrated: §5 typed-enum target + K8s reference + sandbox via path-dep; §6 5-trait ack + federation deferred to v1; §1 convergence row clarifying auth is HTTP not JSON-RPC. Q3 and Q4 resolved. Linked to `arc-relay/docs/rpc-method-inventory.md` for shipped-surface ground truth. |
| *(next)* | *beacon-claude* | *Populate Beacon columns.* |
| *(next)* | *beam-claude* | *Populate Beam columns.* |
| *(next)* | *arc-claude* | *Fill remaining convergence rows after both clients land their columns.* |
