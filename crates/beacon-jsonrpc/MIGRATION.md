# beacon-jsonrpc — Remaining migration targets

This file documents every cross-process boundary that should eventually migrate
to `beacon-jsonrpc` as part of the M3 "transport unification" milestone.

---

## 1. hostd ↔ Mac client  (currently: uniffi events over dylib FFI)

Source files: `beacon-core/crates/beacon-ffi/src/lib.rs`,
`beacon-core/crates/beacon-hostd/src/lib.rs`,
`beacon-mac/Sources/BeaconMac/Bridge/`.

### Methods (request/response)

| Method name             | Params shape                              | Result shape                  | Source fn / type          | Status |
|-------------------------|-------------------------------------------|-------------------------------|---------------------------|--------|
| `session.create`        | `{workspace_id, shell, cwd}`              | `{session_id}`                | `FfiHostd::create_session`| **DONE** — see beacon-hostd/src/rpc.rs |
| `session.attach`        | `{session_id}`                            | `{ok: bool}`                  | `FfiHostd::attach`        | **DONE** — see beacon-hostd/src/rpc.rs |
| `session.resize`        | `{session_id, cols, rows}`               | `{ok: bool}`                  | `FfiHostd::resize`        | **DONE** — see beacon-hostd/src/rpc.rs |
| `session.write`         | `{session_id, data: string}`              | `{ok: bool}`                  | `FfiHostd::write`         | **DONE** — see beacon-hostd/src/rpc.rs |
| `session.close`         | `{session_id}`                            | `{ok: bool}`                  | `FfiHostd::close_session` | **DONE** — see beacon-hostd/src/rpc.rs |
| `memory.list`           | `{scope}`                                 | `[{filename,name,description,memory_type}]` | `MemoryStore::list`     | **DONE** — see beacon-multi-agent/src/memory/rpc.rs |
| `memory.load`           | `{scope, filename}`                       | `{filename,name,description,memory_type,body}` | `MemoryStore::load`  | **DONE** — see beacon-multi-agent/src/memory/rpc.rs |
| `memory.save`           | `{scope,name,description,memory_type,body}` | `{filename}`                | `MemoryStore::save`       | **DONE** — see beacon-multi-agent/src/memory/rpc.rs |
| `memory.delete`         | `{scope, filename}`                       | `{ok: true}`                  | `MemoryStore::delete`     | **DONE** — see beacon-multi-agent/src/memory/rpc.rs |
| `memory.retrieval`      | `{}`                                      | `[{filename,name,description,memory_type,body,score,scope}]` | `MemoryStore::last_retrieval` | **DONE** — see beacon-multi-agent/src/memory/rpc.rs |
| `ai.session.create`     | `{session_id, model}`                     | `{ok: true}`                  | `AiSessionCreateHandler` | **DONE** — see beacon-multi-agent/src/ai_rpc.rs |
| `ai.send_message`       | `{session_id, message}`                   | `{ok: true}` (streams via notifs) | `AiSendMessageHandler` | **DONE** — see beacon-multi-agent/src/ai_rpc.rs |
| `command.describe`      | `{command: string}`                       | `{description: string}`       | **DONE** — see beacon-command/src/rpc.rs | **DONE** |

### Notifications (server → client, no response)

| Notification name        | Params shape                              | Emitted when                  | Status |
|--------------------------|-------------------------------------------|-------------------------------|--------|
| `session.output`         | `{session_id, data: string}`              | PTY produces bytes            | **DONE** — `emit_output()` in beacon-hostd/src/rpc.rs |
| `session.title_changed`  | `{session_id, title: string}`             | OSC 0/2 sequence              | **DONE** — `emit_title_changed()` in beacon-hostd/src/rpc.rs |
| `session.cwd_changed`    | `{session_id, cwd: string}`               | OSC 7 sequence                | **DONE** — `emit_cwd_changed()` in beacon-hostd/src/rpc.rs |
| `session.block_end`      | `{session_id, block: Block}`              | OSC 133;D closes a block      | **DONE** — `emit_block_end()` in beacon-hostd/src/rpc.rs |
| `ai.chunk`               | `{session_id, delta: string}`             | Streaming AI token            | **DONE** — `emit_chunk()` in beacon-multi-agent/src/ai_rpc.rs |
| `ai.done`                | `{session_id, model, content}`            | AI response complete          | **DONE** — `emit_done()` in beacon-multi-agent/src/ai_rpc.rs |
| `ai.error`               | `{session_id, error: string}`             | AI call failed                | **DONE** — `emit_error()` in beacon-multi-agent/src/ai_rpc.rs |

### Notification channel design (feat/jsonrpc-hostd)

Notifications are delivered via `tokio::sync::broadcast::Sender<String>` held inside
each `SessionEntry` in `beacon-hostd::rpc::SessionStore`. Each notification string is
a fully-serialised JSON-RPC 2.0 `Notification` object.

Callers subscribe by calling `FfiHostdRpc::subscribe_notifications(session_id)` which
returns an `Arc<FfiNotifReceiver>`. They poll it with `FfiNotifReceiver::poll_next()`.
This polling approach was chosen over FFI callbacks to avoid the cross-thread
complexity of callback interfaces when the notification may fire from any async task.

### ai.* status (feat/jsonrpc-ai)

`ai.session.create`, `ai.send_message`, plus the three streaming notifications
(`ai.chunk`, `ai.done`, `ai.error`) shipped on `feat/jsonrpc-ai`. Handlers in
`beacon-multi-agent/src/ai_rpc.rs` registered unconditionally. Streaming-over-RPC
design: `ai.send_message` returns `{ok:true}` immediately, then a spawned task
drives `Provider::stream_chat` and emits `ai.chunk` per `StreamEvent::Text` and
either `ai.done` or `ai.error` as the terminal frame. Subscribers attach via
`AiSessionStore::subscribe(session_id)` to a per-session
`tokio::sync::broadcast::Sender<String>`.

`StreamEvent::ToolUse` now emits an `ai.tool_use` notification
(`{session_id, id, name, arguments}`) on the same broadcast channel as
`ai.chunk` / `ai.done` / `ai.error` (shipped on `feat/jsonrpc-ai-tool-events`).
The arc surface emits the same notification name via `ArcAiStreamEvent::ToolUse`
so watchers subscribe identically for local vs Arc. Agent tool flows still use
`FfiAiSession::run_agent` (uniffi) for execution dispatch — RPC tool events are
informational (chip rendering / observability) until the agent loop itself
moves over.

### memory.* status (feat/jsonrpc-memory)

`memory.list`, `memory.load`, `memory.save`, `memory.delete`, `memory.retrieval`
shipped on `feat/jsonrpc-memory`. Handlers in `beacon-multi-agent/src/memory/rpc.rs`
registered unconditionally. Uniffi `memory::ffi` surface kept in parallel
until callers cut over.

---

## 2. Arc WebSocket protocol  (JSON-RPC 2.0 sole transport since Phase G, 2026-05-05)

Source files: `arc-relay/src/relay.rs`, `arc-relay/src/session.rs`,
`beacon-core/crates/beacon-session/src/arc_client.rs`.

### Methods

| Method name                | Params shape                              | Notes                         | Status |
|----------------------------|-------------------------------------------|-------------------------------|--------|
| `arc.session.attach`       | `{session_id, token}`                     | join mirrored session         | **DONE** — `ArcSessionAttachHandler` in arc-relay/src/rpc.rs |
| `arc.session.control`      | `{session_id, requester_id}`              | claim write control           | **DONE** — `ArcSessionControlHandler` |
| `arc.session.release`      | `{session_id}`                            | release write control         | **DONE** — `ArcSessionReleaseHandler` |
| `arc.ai.chat`              | `{model, messages[]}`                     | non-streaming chat fan-out    | **DONE** — `ArcAiChatHandler` in arc-relay/src/ai_rpc.rs |
| `arc.ai.stream_chat`       | `{session_id, model, messages[]}`         | streaming via ai.chunk/ai.done/ai.error | **DONE** — `ArcAiStreamChatHandler` |
| `arc.ai.list_models`       | `{}`                                      | enumerate available models    | **DONE** — `ArcAiListModelsHandler` |

### Notifications

| Notification name           | Params shape                              | Direction     | Status |
|-----------------------------|-------------------------------------------|---------------|--------|
| `arc.mirror.frame`          | `{session_id, data: bytes_b64}`           | relay→watcher | **DONE** — `emit_mirror_frame()` in arc-relay/src/rpc.rs |
| `arc.mirror.control_grant`  | `{session_id, holder_id}`                 | relay→all     | **DONE** — `emit_mirror_control_grant()`; auto-fired by `arc.session.control` |
| `arc.mirror.control_revoke` | `{session_id}`                            | relay→all     | **DONE** — `emit_mirror_control_revoke()`; auto-fired by `arc.session.release` (when a holder existed) |
| `arc.session.participant`   | `{session_id, user_id, joined: bool}`     | relay→all     | **DONE** — `emit_participant()` in arc-relay/src/rpc.rs |

Reference: `arc-relay/docs/ai-route.md` (P3 deferred endpoint spec).

### arc.session.* status (feat/jsonrpc-arc-session)

`arc.session.attach`, `arc.session.control`, `arc.session.release`, plus the
`arc.session.participant` notification shipped on `feat/jsonrpc-arc-session`.
Handlers in `arc-relay/src/rpc.rs` registered unconditionally via
`register`. Per-session state lives in `ArcSessionStore` (controller
holder + per-session `broadcast::Sender<String>` for notifications). The
existing msgpack `ClientToArc`/`ArcToClient` envelope through
`ws/handler.rs` is unchanged; the jsonrpc handlers are wired into the WS
endpoint via `Sec-WebSocket-Protocol: arc.json-rpc` subprotocol negotiation
(see WS-endpoint cut-over section below).

`ArcSessionAttachHandler::with_user_resolver` lets callers swap the default
FNV-1a token hash for a real JWT decode against the relay's `jwt_secret`;
the rpc layer itself stays free of `jsonwebtoken` so consumers can mount it
without that dep.

### arc.ai.* status (feat/jsonrpc-arc-ai)

`arc.ai.chat`, `arc.ai.stream_chat`, `arc.ai.list_models` shipped on
`feat/jsonrpc-arc-ai`. Handlers in `arc-relay/src/ai_rpc.rs` registered
unconditionally via `register`. The handlers depend on a pluggable
`ArcAiBackend` trait (3 methods: `chat`, `stream_chat`, `list_models`).
Streaming reuses the same `ai.chunk` / `ai.done` / `ai.error` notification
surface as the local `ai.*` slice — watchers subscribe with identical code
regardless of whether the chat ran locally or through Arc.

`arc.ai.stream_chat` requires a `session_id` parameter (added beyond the
original method-table sketch); without it concurrent streams would
interleave on a single broadcast channel. Per-session state lives in
`ArcAiSessionStore` (just a per-session `broadcast::Sender<String>`).

### WS-endpoint cut-over — DONE (feat/jsonrpc-ws-cutover)

`Sec-WebSocket-Protocol: arc.json-rpc` is **mandatory** since Phase G
(2026-05-05). Absent header → `400 Bad Request` (msgpack fallback
deleted). The JSON-RPC pump in `arc-relay/src/ws/jsonrpc_handler.rs`
is the sole WebSocket handler.

Live methods over WS: `arc.session.{attach,control,release}`,
`arc.ai.{chat,stream_chat,list_models}`.

Live notifications over WS: `arc.session.participant`,
`arc.mirror.{frame,control_grant,control_revoke}`,
`ai.{chunk,done,error,tool_use}`.

Mirror fanout: `MirrorRegistry::broadcast` invokes `ArcMirrorEmitter`
(`arc-relay/src/rpc.rs`) which maps `ArcToClient::{SessionOutput,
LeaseChanged}` → matching `arc.mirror.*` notifications on the
per-session `broadcast::Sender<String>` watched by the JSON-RPC pump.

~~Production `HttpAiBackend` wiring against `arc-relay/src/api/ai_routes.rs`
is deferred~~ — DONE in `feat/http-ai-backend`; see new section below.

### HttpAiBackend production adapter — DONE (feat/http-ai-backend)

`arc-relay/src/ai_backend.rs::HttpAiBackend` now drives `arc.ai.{chat,stream_chat,list_models}` against the Anthropic Messages API. Wired into `AppState.arc_ai_backend` in `arc-relay/src/main.rs`. Reads `ANTHROPIC_API_KEY` from env at each call (matches legacy `/v1/ai/*` semantics). Provider prefix `anthropic:` strips before forwarding `model` to upstream. Other prefixes return `Err`.

**Out of scope (gaps with `/v1/ai/chat`):**
- ~~`tools` / function calling~~ — DONE in `feat/arc-ai-request-expansion`
- ~~explicit `temperature` / `max_tokens`~~ — DONE in `feat/arc-ai-request-expansion`
- providers other than Anthropic

### `arc.ai.*` request expansion — DONE (feat/arc-ai-request-expansion)

`ArcAiChatRequest` now carries `system`, `tools`, `temperature`, `max_tokens`. `ArcAiMessage` carries `tool_call_id` + `tool_calls[]` for full tool roundtrip. `ArcAiBackend::chat` returns `ArcAiChatResponse { content?, tool_calls[] }` so non-streaming callers can read tool_use blocks. `HttpAiBackend::build_anthropic_request` honors all new fields and emits Anthropic's `tool_use` / `tool_result` content-block format.

Wire shape on `arc.ai.chat` response: `{content?: string, tool_calls?: [{id, type, function:{name, arguments}}]}`. Empty `content` + populated `tool_calls` matches OpenAI convention.

Still out of scope:
- providers other than Anthropic

### Arc surface — Phase A (feat/arc-jsonrpc-phase-a)

Watcher-side session-control surface added (additive; msgpack pump unchanged).

**Methods (5):** `arc.session.{detach,subscribe,unsubscribe,input,resize}`

| Method                    | Params                                                          | Result        |
|---------------------------|-----------------------------------------------------------------|---------------|
| `arc.session.detach`      | `{session_id, token}`                                           | `{ok: true}`  |
| `arc.session.subscribe`   | `{session_id, token}`                                           | `{ok: true}`  |
| `arc.session.unsubscribe` | `{session_id, token}`                                           | `{ok: true}`  |
| `arc.session.input`       | `{session_id, requester_id, data: <base64>}`                    | `{ok: true}`  |
| `arc.session.resize`      | `{session_id, requester_id, cols: u32, rows: u32}`              | `{ok: true}`  |

**Notifications (4):**

| Notification                         | Params                                                  | Direction       |
|--------------------------------------|---------------------------------------------------------|-----------------|
| `arc.session.control_requested`      | `{session_id, request_id, from_user, from_device}`      | relay→holder    |
| `arc.session.gone`                   | `{session_id, reason}`                                  | relay→watchers  |
| `arc.session.revoked`                | `{session_id, reason}`                                  | relay→watcher   |
| `arc.mirror.discontinuity`           | `{session_id, last_seq}`                                | relay→watchers  |

`arc.session.input` and `arc.session.resize` are gated on the caller being the
current write-control holder; they emit `arc.host.input` and
`arc.host.resize_request` notifications respectively. The host-side dispatch
that consumes these notifications lands in Phase D.

### Arc surface — Phase B (feat/arc-jsonrpc-phase-b)

Watcher-side block-stream + replay surface added (additive; msgpack pump unchanged).

**Methods:** none (notification-only phase).

**Notifications (6):**

| Notification                | Params                                                                                                         | Direction      |
|-----------------------------|----------------------------------------------------------------------------------------------------------------|----------------|
| `arc.mirror.block_start`    | `{session_id, header: <BlockHeader-json>}`                                                                     | relay→watcher  |
| `arc.mirror.block_chunk`    | `{session_id, block_id, seq: u64, data: <base64>}`                                                             | relay→watcher  |
| `arc.mirror.block_end`      | `{session_id, footer: <BlockFooter-json>}`                                                                     | relay→watcher  |
| `arc.mirror.replay_begin`   | `{session_id, total_blocks: u32}`                                                                              | relay→watcher  |
| `arc.mirror.replay_block`   | `{session_id, header: <BlockHeader-json>, output: <base64>, footer: <BlockFooter-json \| null>}`               | relay→watcher  |
| `arc.mirror.replay_end`     | `{session_id}`                                                                                                 | relay→watcher  |

`<BlockHeader-json>` and `<BlockFooter-json>` are direct `serde_json::to_value(&BlockHeader)` / `serde_json::to_value(&BlockFooter)` — see `beacon-model::{BlockHeader, BlockFooter}`. Wire keys are pinned by lock tests in `arc-relay/src/rpc.rs::tests::block_header_json_shape_locked` / `block_footer_json_shape_locked`. `replay_block` flattens `Block` so `output` lands as a base64 string at top level (matches the `arc.mirror.frame` byte-payload convention).

The host-side producers that fire these notifications (mapping `BlockEvent` from `beacon-blocks::BlockDetector` into the rpc emitters) land in Phase D.

### Arc surface — Phase C (feat/arc-jsonrpc-phase-c)

Watcher-side approve/deny + host-input echo (additive; msgpack pump unchanged).

**Methods (2):**

| Method                          | Params                                                       | Result        | Direction          |
|---------------------------------|--------------------------------------------------------------|---------------|--------------------|
| `arc.session.approve_control`   | `{session_id, request_id, requester_id}`                     | `{ok: true}`  | watcher (owner)→relay |
| `arc.session.deny_control`      | `{session_id, request_id, requester_id}`                     | `{ok: true}`  | watcher (owner)→relay |

**Notifications (1):**

| Notification              | Params                                                       | Direction       |
|---------------------------|--------------------------------------------------------------|-----------------|
| `arc.mirror.host_input`   | `{session_id, data: <base64>, from_user: <string>}`          | relay→watcher   |

`approve_control` promotes `requester_id` to the new write-control holder and fires `arc.mirror.control_grant`. `deny_control` is silent (no notification — the requesting watcher times out client-side, matching msgpack semantic). The `request_id` parameter is accepted for wire compatibility with msgpack `ClientToArc::ApproveControl` / `DenyControl` and logged but not validated against any server-side pending-request queue — the full lease-manager flow (queue, owner authorization, race resolution) lives behind msgpack until Phase D wires the host surface.

`arc.mirror.host_input` is new logical behavior on the JSON-RPC side: it broadcasts a base64-encoded echo of the current driver's input bytes to all subscribed watchers, attributed via `from_user`. msgpack land does not broadcast `HostInput` to watchers (only forwards to the host PTY); Phase C adds this echo so non-driver watchers see real-time typing attribution. Fired alongside `arc.host.input` from inside `ArcSessionInputHandler`.

The host-side dispatch that consumes the `arc.host.input` and `arc.host.resize_request` notifications (Phase A emitters) lands in Phase D.

---

### Arc surface — Phase D (host surface)

Adds the `arc.host.*` host-facing method surface (9 methods) and one new
relay→host notification (`arc.host.shadow_diff_request`). Two of the spec's
three host-facing notifs (`arc.host.input`, `arc.host.resize_request`)
already shipped as Phase A side effects of `arc.session.{input, resize}`.
Additive — msgpack `handle_host` pump in `arc-relay/src/ws/host.rs` is
untouched.

#### Methods

| Method                              | Params                                                                                | Result                                            |
|-------------------------------------|---------------------------------------------------------------------------------------|---------------------------------------------------|
| `arc.host.hello`                    | `{host_id, auth_token, protocol_version}`                                             | `{ok: true, protocol_version}`                    |
| `arc.host.session_register`         | `{session_id, name}`                                                                  | `{ok: true}`                                      |
| `arc.host.session_unregister`       | `{session_id}`                                                                        | `{ok: true}`                                      |
| `arc.host.block_start`              | `{session_id, header: <BlockHeader-json>}`                                            | `{ok: true}`                                      |
| `arc.host.block_chunk`              | `{session_id, block_id, seq, data: <base64>}`                                         | `{ok: true}`                                      |
| `arc.host.block_end`                | `{session_id, footer: <BlockFooter-json>}`                                            | `{ok: true}`                                      |
| `arc.host.session_resize`           | `{session_id, cols, rows}`                                                            | `{ok: true}`                                      |
| `arc.host.push_output`              | `{session_id, data: <base64>, seq}`                                                   | `{ok: true, seq}`                                 |
| `arc.host.shadow_diff_response`     | `{session_id, request_id, diff}`                                                      | `{ok: true}`                                      |

#### New notification

| Notification                        | Params                                       | Direction       |
|-------------------------------------|----------------------------------------------|-----------------|
| `arc.host.shadow_diff_request`      | `{session_id, request_id}`                   | relay→host      |

#### Semantics

- **`arc.host.hello`** — connection-level handshake. Validates `protocol_version`
  matches `beacon_proto::PROTOCOL_VERSION`; rejects with `server_error` on
  mismatch. Auth token is logged but not verified (WS upgrade handler is auth
  boundary).
- **`arc.host.session_register`** — `get_or_create` the entry; emits no notif
  (matches msgpack — host registration is invisible to existing watchers).
- **`arc.host.session_unregister`** — fires `arc.session.gone` with
  `reason: "host_unregistered"`; idempotent (returns `{ok: true}` on missing
  entry).
- **`arc.host.block_start` / `block_chunk` / `block_end`** — re-broadcast
  to subscribed watchers via Phase B's `arc.mirror.block_*` emitters.
- **`arc.host.session_resize`** — host-driven resize. In-rpc-layer no-op
  (logs at debug, returns `{ok: true}`); Phase F wires `RelayRouter::update_size`.
- **`arc.host.push_output`** — re-broadcasts as `arc.mirror.frame`. Echoes
  `seq` in result for host-side correlation.
- **`arc.host.shadow_diff_response`** — in-rpc-layer no-op; Phase F wires
  the waiter map (`RelayRouter::pending_shadow_diffs`).

#### In-rpc-layer convention

All 9 handlers act only on `ArcSessionStore.entry`. No `RelayRouter` /
`MirrorRegistry` / DB interaction in this phase. Phase F (Mac client wire
migration) will refactor the handlers to wire to production routing /
persistence; the wire shapes locked here are stable across that refactor.

#### What ships next

Phase E migrates the arc-web TS client to `JsonRpcWsClient`. Phase F migrates
the Mac client (`beacon-net::ArcConnection`). Phase G deletes the msgpack
envelope.

### Arc surface — Phase E (arc-web wire migration)

Switches arc-web's `/sessions/[id]` (attach) and `/sessions/[id]/watch`
(mirror) pages from the msgpack `useWebSocket` hook to the new
`useArcJsonRpc` hook backed by `JsonRpcWsClient`. **TS-only.** Server side
unchanged. Msgpack stack (`ws.ts`, `use-websocket.ts`, `protocol.ts`) stays
in tree — orphaned but available for the Mac client cut-over (Phase F)
which does not touch arc-web. Phase G deletes msgpack on both sides.

#### What moved

| Page | Method calls | Notifications consumed |
|------|--------------|------------------------|
| `/sessions/[id]` | `arc.session.{attach,detach,input,resize,control,release,approve_control,deny_control}` | `arc.mirror.{block_*,replay_*,control_{grant,revoke}}`, `arc.session.{participant,control_requested,gone}` |
| `/sessions/[id]/watch` | `arc.session.{subscribe,unsubscribe,control,release,approve_control,deny_control}` | `arc.mirror.{frame,discontinuity,host_input,control_{grant,revoke}}`, `arc.session.{participant,revoked,control_requested}` |

#### Block decoding on /watch

Mirror stream still feeds the `arc-web-blocks` WASM detector. Bytes now
arrive base64-encoded in `arc.mirror.frame.data`; the page does
`atob` → `Uint8Array` → existing `useMirrorBlocks.feedBytes(...)`. No
WASM-side changes.

#### What ships next

Phase F migrates the Mac client (`beacon-net::ArcConnection`). Phase G
deletes the msgpack envelope on both sides.

### Arc surface — Phase F (beacon-net + production wire-up)

Two halves shipped together:

**A. arc-relay production wire-up.** The nine `arc.host.*` handlers from
Phase D were dispatcher-only stubs; Phase F connects them to the same
production state the msgpack `ws/host.rs` path uses. JSON-RPC hosts now
fully participate in mirroring, presence, and shadow-diff request/response.

| Handler | Wire-up |
|---------|---------|
| `arc.host.session_register` | `RelayRouter::register_session` + `MirrorRegistry::get_or_init` + host_input pump + DB `create_session` |
| `arc.host.session_unregister` | `RelayRouter::unregister_session` + DB `update_session_state(_, "ended")` + abort translator + pump |
| `arc.host.session_resize` | `RelayRouter::update_size` |
| `arc.host.push_output` | `MirrorRegistry::scrollback.append` (canonical seq) → `mirror_registry.broadcast` |
| `arc.host.shadow_diff_response` | `RelayRouter::resolve_shadow_diff` |

`shadow_routes::get_shadow_diff` emits `arc.host.shadow_diff_request`
via the JSON-RPC notification channel (Phase F). Phase G removed the
parallel msgpack `send_to_host` call — JSON-RPC is the sole emitter.

Host-bound msgpack frames (sent via `RelayRouter::register_session`'s
`host_tx: mpsc::Sender<Vec<u8>>`) are translated to `arc.host.*` /
`arc.session.*` JSON-RPC notifications by a per-session translator task
spawned at `session_register` and aborted at `session_unregister`. Handles
are stored on `ArcSessionEntry::host_tasks`.

**B. Mac client wire migration.** New
`beacon_net::ArcConnectionJsonRpc` wraps the existing `JsonRpcWsClient`
and exposes the same `send(ClientToArc) / recv() -> ArcToClient` surface
as msgpack `ArcConnection` so `beacon-hostd::daemon` can switch via
the `BEACON_FF_JSON_RPC_SPINE` feature flag without rewriting its
match arms. Outbound `ClientToArc` is method-mapped (e.g.
`SessionRegister` → `arc.host.session_register`); inbound `arc.*`
notifs are reverse-mapped (e.g. `arc.host.input` → `HostInput`,
`arc.mirror.control_grant`/`revoke` → `LeaseChanged`).

The daemon's `ArcWire` enum (private to `daemon.rs`) wraps both
connection types so `relay_loop`, `mirror_upstream_loop`, and
`inbound_loop` accept either implementation.

**Self-subscribe filter** is client-side: `ArcConnectionJsonRpc`'s
translator drops `arc.mirror.frame`, `arc.mirror.block_*`,
`arc.mirror.replay_*`, `arc.mirror.discontinuity`, and
`arc.mirror.host_input` since hostd has no mirror-render need.
`arc.mirror.control_grant`/`revoke` are PRESERVED (mapped to
`LeaseChanged`) because the daemon's lease cache needs them.

~~#### Known gap — JSON-RPC reconnect parity~~

~~`JsonRpcWsClient` has no auto-reconnect; the WS actor breaks on close.
msgpack `ArcConnection` reconnects with backoff via its own actor loop.
Production hostd JSON-RPC connections will NOT survive WS drops until a
follow-up lands either (a) a reconnect wrapper around
`ArcConnectionJsonRpc` modeled on the msgpack actor, or (b) wires the
WS actor's exit path to drop `notif_tx` so `state_watch` publishes
`Disconnected` and `beacon-hostd::daemon::relay_loop` can react.
Test `state_stays_connected_after_drop_documenting_no_autoreconnect`
locks in the current behavior as a tripwire.~~

**Closed** — see Reconnect parity section below.

### Arc surface — Phase G (msgpack deletion, 2026-05-05)

**Terminal state.** JSON-RPC 2.0 over WebSocket is now the sole Arc
transport. All msgpack wire code deleted.

#### Deleted

| Component | What was removed |
|-----------|-----------------|
| `beacon-net::ArcConnection` | Entire msgpack WS client (`src/connection.rs`, ~465 lines) |
| `beacon-net::ArcWire` enum | Private `daemon.rs` enum that wrapped both transport types |
| `arc-relay/src/ws/host.rs` | Msgpack host-facing WS dispatcher (~490 lines) |
| `arc-relay/src/ws/client.rs` | Msgpack watcher-facing WS dispatcher (~588 lines) |
| `arc-relay/src/ws/handler.rs::handle_connection` | Msgpack WS upgrade pump |
| `arc-relay/src/api/shadow_routes.rs` (partial) | Parallel msgpack `send_to_host` call; JSON-RPC path is now sole emitter |
| `arc-web/src/lib/ws.ts` | Msgpack encode/decode + `connectToRelay` |
| `arc-web/src/types/protocol.ts` | `ClientToArc` / `ArcToClient` msgpack types |
| `arc-web/src/hooks/use-websocket.ts` | React hook wrapping `lib/ws.ts` |
| `arc-web/src/hooks/use-blocks.ts` | React hook consuming `ArcToClient` events |

#### Kept (intentionally retained for back-compat)

- `ConnectionState` enum — moved from deleted `connection.rs` to `beacon-net::config` (shared by `jsonrpc/client.rs` and `jsonrpc_connection.rs`).
- `beacon-hostd::rpc::register` — previously `register_if_enabled`; `JsonRpcSpine` gate removed, returns `()`.

#### Cleanup tail (post-Phase G, 2026-05-06)

`FeatureFlag::JsonRpcSpine` has been **removed** from the `beacon-features` enum.
All formerly-gated `register_if_enabled` helpers were renamed to `register` and
the `is_enabled(JsonRpcSpine)` checks were dropped — handlers now register
unconditionally. `BEACON_FF_JSON_RPC_SPINE` env-var and `json_rpc_spine` TOML
keys are silently ignored by `from_str_key` (unknown key → `None`). No panic,
no data loss.

#### Subprotocol enforcement

`ws_handler` returns `400 Bad Request` when `Sec-WebSocket-Protocol: arc.json-rpc` is absent. All production clients (Mac hostd via `beacon-net::ArcConnectionJsonRpc`, arc-web via `JsonRpcWsClient`) send this header.

#### Token revalidation

`handle_jsonrpc_connection` now runs the same periodic `TOKEN_REVALIDATE_INTERVAL` expiry check the deleted msgpack `handle_connection` had. Close codes: 4001 (expired), 4003 (invalid). Integration tests in `arc-relay/tests/ws_jsonrpc_token_expiry.rs` gated on `--features fast-revalidate`.

### Reconnect parity (post-Phase F)

Closes the reconnect gap documented in Phase F. Landed on
`feat/jsonrpc-reconnect-parity`.

#### What closed

**`JsonRpcWsClient` now auto-reconnects with exponential backoff.**

The WS actor (`connection_actor`) wraps the inner `run_message_loop` in an
outer reconnect loop mirroring msgpack `connection::connection_actor`:

- On disconnect: drain `pending` (drops senders → callers see `Shutdown`),
  then enter exponential-backoff retry (`backoff::delay` + `backoff::apply_jitter`).
- Each retry: publish `ConnectionState::Reconnecting { attempt }` → sleep
  jittered delay → call `open_and_negotiate`; on success publish `Connected`.
- `state_tx`/`state_rx` created BEFORE actor spawn, initialized to `Connected`.
- New fields on `JsonRpcWsConfig`: `initial_backoff` (100ms), `max_backoff` (30s),
  `jitter_factor` (0.25). Builder: `with_reconnect(initial, max, jitter)`.

**`ArcConnectionJsonRpc` now forwards client state; re-issues `arc.host.hello` on reconnect.**

- Dropped the dead local `watch::channel(Connected)` that was never updated.
  `state_rx` now delegates to `client.state_watch()` — real transitions propagate.
- Supervisor task watches `state_watch()` and re-issues `arc.host.hello` on each
  `Reconnecting → Connected` transition. A failed re-handshake is logged as warning
  and does not abort (next caller request fails and triggers another cycle).
- `_supervisor: JoinHandle<()>` stored on the struct so it lives as long as the
  connection; aborted in `close()`.
- `NetConfig::initial_backoff / max_backoff / jitter_factor` are now threaded
  through to `JsonRpcWsConfig::with_reconnect()` — previously `_config` was silently
  ignored.

**`hostd::daemon::relay_loop` already handles the state transitions.**

`relay_loop` watches `conn.state_watch()` and calls `registry.on_reconnect()` on
`Connected`-after-non-Connected. This code path now fires for JSON-RPC connections —
sessions are re-registered after a WS drop without any additional relay_loop changes.

#### Tests added

| File | Tests |
|------|-------|
| `beacon-net/src/jsonrpc/backoff.rs` | 4 unit tests (doubling, cap, zero-attempt, jitter window) |
| `beacon-net/tests/jsonrpc_client.rs` | `client_state_flips_to_reconnecting_on_server_drop`, `client_state_returns_to_connected_after_server_restart` |
| `beacon-net/tests/jsonrpc_connection.rs` | `state_flips_to_reconnecting_after_server_drops`, `state_returns_to_connected_after_server_restart` (replaced tripwire `state_stays_connected_after_drop_documenting_no_autoreconnect`) |
| `beacon-hostd/tests/jsonrpc_reconnect_smoke.rs` | `relay_loop_sends_session_register_after_jsonrpc_reconnect` |

---

## 3. beacon-remote  (SSH transport)

Source files: `beacon-core/crates/beacon-remote/src/lib.rs`,
`beacon-core/crates/beacon-remote/src/rpc.rs`.

Branch: `feat/jsonrpc-remote` (rpc spine landed today).

### Methods

| Method name                | Params shape                              | Notes                             | Status |
|----------------------------|-------------------------------------------|-----------------------------------|--------|
| `remote.connect`           | `{host, port?, user, auth: {type, ...}}` | allocates entry + broadcast chan; transport spin-up belongs to caller layer | **DONE** — `RemoteConnectHandler` in beacon-remote/src/rpc.rs |
| `remote.disconnect`        | `{remote_session_id}`                    | emits final `remote.disconnected` then removes entry | **DONE** — `RemoteDisconnectHandler` |
| `remote.exec`              | `{remote_session_id, command}`           | run one-off command (acks `{ok:true}`; backend emits `remote.pty_data` + `remote.exit_status`) | **DONE** (rpc layer) — `RemoteExecHandler` + pluggable `RemoteExecBackend` trait; russh-side `Channel::exec()` wiring still pending |
| `remote.file.read`         | `{remote_session_id, path}`              | SFTP read                         | advertised, server_error stub — needs russh-sftp |
| `remote.file.write`        | `{remote_session_id, path, content_b64}` | SFTP write                        | advertised, server_error stub — needs russh-sftp |
| `remote.forward.start`     | `{remote_session_id, local_port, remote_port}` | port forward (deferred)   | advertised, server_error stub — needs russh forward channel |

`auth.type` accepts `password` (with `password`), `private_key` (with
`key_path`, optional `passphrase`), or `agent` (no extra fields). Unknown
types fail with invalid_params.

### Notifications

Names match the live [`RemoteEvent`] enum; the original draft's `remote.output`
was renamed to `remote.pty_data` to match the existing transport docs.

| Notification name          | Params shape                              | Direction         | Status |
|----------------------------|-------------------------------------------|-------------------|--------|
| `remote.connected`         | `{remote_session_id}`                    | daemon→client     | **DONE** — `emit_connected()` in beacon-remote/src/rpc.rs |
| `remote.pty_data`          | `{remote_session_id, data_b64}`          | daemon→client     | **DONE** — `emit_pty_data()` (base64-encodes raw bytes) |
| `remote.exit_status`       | `{remote_session_id, code}`              | daemon→client     | **DONE** — `emit_exit_status()` |
| `remote.disconnected`      | `{remote_session_id, reason?}`           | daemon→client     | **DONE** — `emit_disconnected()` |

### Notification channel design (feat/jsonrpc-remote)

Identical to hostd's pattern: each `RemoteSessionEntry` in
`RemoteSessionStore` carries a `tokio::sync::broadcast::Sender<String>` of
JSON-encoded `Notification` strings. A higher-level wiring layer that owns
the runtime and the `ssh` cargo feature reads `entry.notif_tx` and bridges
`SshTransport`'s `mpsc<RemoteEvent>` into `emit_*` calls; the rpc layer
itself stays free of cargo feature gymnastics so consumers can mount it
without compiling russh.

---

## 4. Plugin host  (currently gRPC, NOT migrated here)

Source files: `beacon-core/crates/beacon-plugin-host/`, `beacon-core/crates/beacon-plugin-proto/`.

The plugin protocol uses gRPC (tonic 0.11) over a Unix domain socket. This is a different
protocol family and **will not** be migrated to `beacon-jsonrpc`. The P7 spec explicitly
excludes plugin-host gRPC from the unification scope.

---

## Estimated migration order

1. `command.describe` — **DONE** (branch `refactor/jsonrpc-spine`)
2. `session.*` (5 methods) + 4 session notifications — **DONE** (branch `feat/jsonrpc-hostd`)
   - `SessionRpcHandler` in `beacon-hostd/src/rpc.rs`
   - `FfiHostdRpc` + `FfiNotifReceiver` + `FfiSessionStore` in `beacon-ffi/src/lib.rs`
   - Notification channel: `broadcast::Sender<String>` per session, polled via `FfiNotifReceiver::poll_next()`
3. `memory.*` (list, load, save, delete, retrieval) — **DONE** (branch `feat/jsonrpc-memory`)
4. `ai.*` (`ai.session.create`, `ai.send_message` + 3 streaming notifications) — **DONE** (branch `feat/jsonrpc-ai`)
5. `arc.*` — `arc.session.*` (3 methods + 1 notif) **DONE** (branch `feat/jsonrpc-arc-session`); `arc.mirror.*` (3 notifs) **DONE** (branch `feat/jsonrpc-arc-mirror`); `arc.ai.*` (3 methods) **DONE** (branch `feat/jsonrpc-arc-ai`)
6. `remote.*` — connect/disconnect + 4 notifs **DONE** (branch `feat/jsonrpc-remote`); exec/file.read/file.write/forward.start advertised but stubbed pending russh wiring

Each migration step was feature-flagged (`FeatureFlag::JsonRpcSpine`) until
the full boundary cut over. Phase G (2026-05-05) completed the cut-over and
deleted all msgpack wire code. The cleanup tail (2026-05-06) removed the
`JsonRpcSpine` enum variant entirely — all `register_if_enabled` helpers were
renamed to `register` and the `is_enabled` checks dropped; handlers now
register unconditionally.

---

## beacon-net JSON-RPC client — DONE (2026-05-02)

Added `beacon-net::jsonrpc::JsonRpcWsClient` shipping on `feat/beacon-net-jsonrpc-client`.

- Connects to arc-relay's `/ws` with `Sec-WebSocket-Protocol: arc.json-rpc`.
- Async `request(method, params) -> Value` API with per-request id (`Id::Num`) + oneshot demux.
- `notifications() -> broadcast::Receiver<Notification>` for server-pushed events.
- Subprotocol negotiation enforced (errors out if server didn't accept `arc.json-rpc`).
- Existing msgpack `ArcConnection` left untouched — callers migrate in a follow-up slice.

3 integration tests against fake axum WS server cover request round-trip,
notification fan-out, and RPC error surfacing.
