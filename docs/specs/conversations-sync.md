# `/api/v1/conversations/sync` — schema sketch

**Status:** draft v0.1 (2026-05-16, beam-claude). **Sketch, not contract** — arc-claude has final say on the server-side shape; this is the joint-design starting point promised in REQUIREMENTS.md Q5.
**Closes:** REQUIREMENTS.md §"Decisions still pending" item 5 (sync schema design).
**Deliverable:** B3 from arc-009.
**Source-of-truth local schema:** WarpMod `app/src/ai/agent_loop/store.rs:1-135`.

---

## Goal

Beam users running on multiple devices (laptop + desktop, or laptop + iPad-shaped-future-Beam) want conversations to follow them. They start a chat on one machine and pick it up later on another. The server tier (`arc-relay`) brokers this without becoming the master copy — local SqliteStore stays authoritative on each device; the server is just the rendezvous.

Out of scope for v0: real-time mirroring (that's §3 `arc.session.*`). Out of scope for v0: conflict-resolution UI. Sync is **eventual-consistency, last-write-wins on metadata, union-merge on append-only message rows.**

---

## Beam-side local schema (what we sync)

From `WarpMod/app/src/ai/agent_loop/store.rs:13-30`:

```sql
CREATE TABLE conversations(
  id          TEXT PRIMARY KEY,    -- ULID
  model_id    TEXT NOT NULL,        -- e.g. "claude-sonnet-4-5"
  title       TEXT NOT NULL DEFAULT '',
  agent_kind  TEXT NOT NULL,        -- "local" | "hosted"
  created_at  INTEGER NOT NULL,     -- unix-seconds
  updated_at  INTEGER NOT NULL      -- bumped on any conv mutation
);

CREATE TABLE messages(
  id          TEXT PRIMARY KEY,    -- ULID
  conv_id     TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
  idx         INTEGER NOT NULL,     -- monotonic per conv_id, append-only
  role        TEXT NOT NULL,        -- "system" | "user" | "assistant" | "tool"
  kind        TEXT NOT NULL,        -- "text" | "tool_call" | "tool_result"
  payload     BLOB NOT NULL,        -- prost-encoded warp_multi_agent_api::Message
  created_at  INTEGER NOT NULL,
  UNIQUE(conv_id, idx)
);
```

**Explicitly NOT synced:**

- `permissions` — per-device security policy; users shouldn't carry "allow_always" decisions across machines without re-consenting.
- `provider_credentials` — device-bound (Keychain on macOS); never leaves the device.

---

## Wire shape

One endpoint, bidirectional. Client sends what it has; server returns the deltas the client doesn't have.

### Request: `POST /api/v1/conversations/sync`

```jsonc
{
  "cursor": {
    "last_sync_at": 1747400000        // unix-seconds, or null on first sync
  },
  "uploads": {
    "conversations": [
      {
        "id": "01HX...",                // ULID
        "model_id": "claude-sonnet-4-5",
        "title": "Refactor the auth module",
        "agent_kind": "local",
        "created_at": 1747395000,
        "updated_at": 1747399500,
        "deleted_at": null              // OR an int → server tombstones; client may purge
      }
    ],
    "messages": [
      {
        "id": "01HX...",
        "conv_id": "01HX...",
        "idx": 0,
        "role": "user",
        "kind": "text",
        "payload_b64": "CgwIBxIIaGVsbG8...",    // base64 of the prost blob
        "created_at": 1747395100
      }
      // …
    ]
  },
  "client_meta": {
    "device_id": "uuid-or-stable-hash",          // for audit + later vector-clock work
    "client": "beam",                              // "beam" | "beacon"
    "version": "0.2.1"
  }
}
```

### Response

```jsonc
{
  "cursor": {
    "now": 1747400123                              // client persists; next call's last_sync_at
  },
  "downloads": {
    "conversations": [
      {
        "id": "01HY...",
        "model_id": "claude-sonnet-4-6",
        "title": "Plan the API redesign",
        "agent_kind": "local",
        "created_at": 1747398000,
        "updated_at": 1747400050,
        "deleted_at": null
      }
    ],
    "messages": [
      {
        "id": "01HY...",
        "conv_id": "01HY...",
        "idx": 4,
        "role": "assistant",
        "kind": "text",
        "payload_b64": "…",
        "created_at": 1747400049
      }
    ]
  },
  "applied": {
    "conversations_upserted": 3,
    "conversations_tombstoned": 1,
    "messages_upserted": 27,
    "messages_rejected": 0
  },
  "next_page_token": null                          // non-null → call again with same body + token
}
```

### Headers + auth

- `Authorization: Bearer <jwt>` per §9 wire invariants.
- `Content-Type: application/json`.
- `Content-Encoding: gzip` recommended — payloads with embedded images can be MBs.

---

## Conflict resolution

| Resource | Rule | Rationale |
| --- | --- | --- |
| **Conversation metadata** (title, model_id, agent_kind) | **Last-write-wins on `updated_at`.** Larger timestamp wins; ties broken by `id` lexicographic. | Metadata is small, conflicts are rare, LWW is predictable. |
| **Conversation existence (deleted_at)** | **Tombstone-wins.** If either side has `deleted_at` set, conversation is deleted post-merge. | Users expect "I deleted this on my laptop, it should be gone everywhere." |
| **Messages** | **Append-only union by `(conv_id, idx)`.** Server keeps the first message ever seen at a given `(conv_id, idx)` slot; later writes to the same slot are rejected with `messages_rejected += 1`. | Messages are append-only by the local loop; same slot from two devices is a divergence the user almost certainly didn't intend. |
| **Message payload modification** | **Disallowed.** Once a message at `(conv_id, idx)` exists, its `payload` is immutable. | Conversations are an audit trail; editing past messages is out of scope for v0. |

Divergence path: if Device A and Device B both append at `idx = 17` to the same `conv_id`, they have effectively forked. One will win on first-sync (whichever reaches the server first). The other's local row is overwritten on download. **In v0 we accept this as data loss.** v1 may introduce vector clocks or a "fork conversation" UX.

To minimize the divergence window, the agent loop SHOULD assign `idx` only when the message is persisted server-side too (i.e., sync before increment). Out of scope for v0; deferred to client-side optimization.

---

## Sync cadence

| Trigger | When |
| --- | --- |
| **Cold start** | On first launch after sign-in. Fetches everything with `last_sync_at: null`. May be paginated for users with thousands of conversations. |
| **Periodic** | Every 5 minutes while Beam is foreground. |
| **Manual** | "Sync now" button. |
| **On conversation close** | After the user explicitly archives or stops a conversation, push immediately so the other device sees the result. |
| **On long-running agent finish** | When the agent loop reaches a terminal state (response complete, error, cancelled), push within 5s. |

Out of scope for v0: WebSocket-based real-time sync. Polling is fine until usage data says otherwise.

---

## Pagination

Server returns up to **500 conversations** + **5000 messages** per response. If the working set exceeds that, response includes `next_page_token: "<opaque>"`. Client calls again with the same `cursor` + uploads but adds `page_token` to the request:

```jsonc
{
  "cursor": { "last_sync_at": 1747400000 },
  "page_token": "<opaque from previous response>",
  "uploads": { /* … */ },
  "client_meta": { /* … */ }
}
```

Server-side, `page_token` opaquely encodes (resource_kind, last_id_returned). Clients MUST NOT inspect or modify it.

---

## Size limits (advisory; relay-enforced)

| Limit | Value |
| --- | --- |
| Single conversation in upload | 1000 messages |
| Single request payload | 32 MiB (post-gzip) |
| Single message `payload_b64` decoded | 2 MiB |
| Total messages per response page | 5000 |
| Total conversations per response page | 500 |

Conversations larger than 1000 messages should be split into multiple sync calls. v1 may add server-side conversation summarization to keep working sets bounded.

---

## Error semantics

| Condition | Status | Body |
| --- | --- | --- |
| Auth failed | 401 | `{ "error": "auth_failed" }` |
| Payload too large | 413 | `{ "error": "payload_too_large", "limit_bytes": ... }` |
| Bad cursor / token | 400 | `{ "error": "invalid_cursor" }` |
| Conflict on `(conv_id, idx)` | 200 (success; rejection counted in `applied.messages_rejected`) | included in response |
| Server error | 5xx | retry with exponential backoff |

Idempotency: re-issuing the same request with the same body + `Idempotency-Key` header MUST return the same response. Server holds idempotency cache for 24h.

---

## Privacy & encryption

- **At rest on server:** payloads are encrypted with a per-user key derived from the user's auth state. Server admin cannot read message content without the user's session.
- **In transit:** TLS 1.3 (Traefik termination + Let's Encrypt).
- **At egress to client:** plaintext after decrypt. Client trusts the bearer-token-authenticated channel.
- **Retention:** mirrors REQUIREMENTS.md §"Out of scope" — Arc stores session output for sync but doesn't retain indefinitely. Default retention: 1 year inactive; user-deletable at any time.

---

## What this sketch does NOT cover

These are gaps the joint-design discussion in PR comments should resolve before this becomes a real spec:

1. **Image payloads embedded in messages.** The `payload_b64` field carries prost-encoded `Message`, which may embed images per `docs/specs/message-image-envelope.md`. Need explicit handling for messages whose total bytes exceed the per-message 2 MiB cap — fork to side-channel image storage (R2) and reference by URL?
2. **Tool-call provenance.** Should the server store the tool_call_id + tool name as searchable columns alongside the opaque payload? Useful for "find all conversations where I used `bash` to deploy".
3. **Cross-user conversation sharing.** Out of scope for v0; flagged here so the schema doesn't paint into a corner. The `id` is a ULID, not a UUID, so collision risk for cross-user sharing is low.
4. **Vector-clock conflict resolution.** Sketched above as "v1"; if a real divergence scenario surfaces in beta, may need to land earlier.
5. **Beacon-side equivalent.** This spec is Beam-flavored (uses WarpMod's schema); Beacon may have a different local store that needs different field mappings. The wire shape should be neutral.

---

## Open questions for arc-claude

1. **Endpoint name + verb.** `POST /api/v1/conversations/sync` is a guess. Some prior art uses `PATCH /api/v1/conversations` for delta semantics. Pick whichever matches your existing convention.
2. **Bearer JWT or session token?** The §1 convergence uses JWT for AI/RPC. Sync is a heavier path; should it use a longer-lived session token instead, or is per-call JWT validation acceptable on a 5-min poll interval?
3. **Per-page size caps.** I picked 500 / 5000 as round numbers. If your Postgres + axum + serde-json stack has different sweet spots, override.
4. **Image side-channel.** Listed in §"What this sketch does NOT cover" as item #1 — important enough that arc may want to decide upfront.
5. **`agent_kind: "hosted"` semantics.** If a conversation was started on a cloud agent (per ROADMAP §C1), it's already authoritative on the server. Do those skip sync entirely (server is the truth), or do they sync the same way for local-cache purposes?

---

## References

- WarpMod `app/src/ai/agent_loop/store.rs:1-135` — local schema.
- WarpMod `app/src/ai/agent_loop/store_sqlite.rs` — SQLite-backed store (in-progress port from `InMemoryStore`).
- ARC `REQUIREMENTS.md` §8 "New endpoints Beam needs" — original entry that triggered this spec.
- ARC `REQUIREMENTS.md` §"Decisions still pending" Q5 — schema design promised.
- ARC `docs/specs/message-image-envelope.md` (PR #4) — image envelope referenced by message payloads.

---

**Edit log**

| Date | By | Change |
| --- | --- | --- |
| 2026-05-16 | beam-claude | Initial sketch. Bidirectional one-endpoint sync, LWW on conv metadata, union-merge on messages, tombstone-wins on deletes. Deliverable B3 from arc-009. |
