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
`beacon-multi-agent/src/ai_rpc.rs` gated by `JsonRpcSpine`. Streaming-over-RPC
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
gated by `JsonRpcSpine` flag. Uniffi `memory::ffi` surface kept in parallel
until callers cut over.

---

## 2. Arc WebSocket protocol  (currently: msgpack over WebSocket)

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
Handlers in `arc-relay/src/rpc.rs` gated by `JsonRpcSpine` via
`register_if_enabled`. Per-session state lives in `ArcSessionStore` (controller
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
`feat/jsonrpc-arc-ai`. Handlers in `arc-relay/src/ai_rpc.rs` gated by
`JsonRpcSpine` via `register_if_enabled`. The handlers depend on a pluggable
`ArcAiBackend` trait (3 methods: `chat`, `stream_chat`, `list_models`).
Streaming reuses the same `ai.chunk` / `ai.done` / `ai.error` notification
surface as the local `ai.*` slice — watchers subscribe with identical code
regardless of whether the chat ran locally or through Arc.

`arc.ai.stream_chat` requires a `session_id` parameter (added beyond the
original method-table sketch); without it concurrent streams would
interleave on a single broadcast channel. Per-session state lives in
`ArcAiSessionStore` (just a per-session `broadcast::Sender<String>`).

### WS-endpoint cut-over — DONE (feat/jsonrpc-ws-cutover)

`Sec-WebSocket-Protocol: arc.json-rpc` selects the JSON-RPC pump in
`arc-relay/src/ws/jsonrpc_handler.rs`. Absent header → existing msgpack
pump in `arc-relay/src/ws/handler.rs::handle_connection`, unchanged.

Live methods over WS: `arc.session.{attach,control,release}`,
`arc.ai.{chat,stream_chat,list_models}`.

Live notifications over WS: `arc.session.participant`,
`arc.mirror.{frame,control_grant,control_revoke}`,
`ai.{chunk,done,error,tool_use}`.

Mirror fanout: `MirrorRegistry::broadcast` invokes registered
`BroadcastEmitter`s alongside the existing msgpack `Fanout::broadcast` path.
The default emitter (`ArcMirrorEmitter` in `arc-relay/src/rpc.rs`) maps
`ArcToClient::{SessionOutput, LeaseChanged}` → matching `arc.mirror.*`
notifications on the per-session `broadcast::Sender<String>` watched by
the JSON-RPC pump. Both wire formats fan out from the same call site
(`src/ws/host.rs::PushOutput`).

~~Production `HttpAiBackend` wiring against `arc-relay/src/api/ai_routes.rs`
is deferred~~ — DONE in `feat/http-ai-backend`; see new section below.

Client adoption (Mac via `beacon-net`, arc-web watch page) lands in a
follow-up plan once the wire path settles in production. `JsonRpcSpine`
flag flip + msgpack-path removal is a separate cleanup slice.

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

1. `command.describe` — **DONE** (branch `refactor/jsonrpc-spine`, gated by `FeatureFlag::JsonRpcSpine`)
2. `session.*` (5 methods) + 4 session notifications — **DONE** (branch `feat/jsonrpc-hostd`)
   - `SessionRpcHandler` in `beacon-hostd/src/rpc.rs`
   - `FfiHostdRpc` + `FfiNotifReceiver` + `FfiSessionStore` in `beacon-ffi/src/lib.rs`
   - Notification channel: `broadcast::Sender<String>` per session, polled via `FfiNotifReceiver::poll_next()`
3. `memory.*` (list, load, save, delete, retrieval) — **DONE** (branch `feat/jsonrpc-memory`, gated by `JsonRpcSpine`)
4. `ai.*` (`ai.session.create`, `ai.send_message` + 3 streaming notifications) — **DONE** (branch `feat/jsonrpc-ai`, gated by `JsonRpcSpine`)
5. `arc.*` — `arc.session.*` (3 methods + 1 notif) **DONE** (branch `feat/jsonrpc-arc-session`); `arc.mirror.*` (3 notifs) **DONE** (branch `feat/jsonrpc-arc-mirror`); `arc.ai.*` (3 methods) **DONE** (branch `feat/jsonrpc-arc-ai`)
6. `remote.*` — connect/disconnect + 4 notifs **DONE** (branch `feat/jsonrpc-remote`); exec/file.read/file.write/forward.start advertised but stubbed pending russh wiring

Each migration step should remain feature-flagged (`FeatureFlag::JsonRpcSpine`) until
the full boundary is cut over, then the flag can be removed.

### `JsonRpcSpine` default → ON (2026-05-02)

`FeatureFlag::JsonRpcSpine.builtin_default()` now returns `true` (release + debug). All
`register_if_enabled` call sites continue to honour env (`BEACON_FF_JSON_RPC_SPINE=0`)
and TOML overrides, so callers can still disable the spine if needed. Removal of the
gating helpers (turning `register_if_enabled` into unconditional `register`) is a
follow-up cleanup once the wire-side cut-over (msgpack → JSON-RPC) is complete.

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
