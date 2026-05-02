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

`StreamEvent::ToolUse` is currently dropped in this surface — agent tool flows
continue to use `FfiAiSession::run_agent` (uniffi) until a future slice
threads tool events through the RPC channel.

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

| Method name                | Params shape                              | Notes                         |
|----------------------------|-------------------------------------------|-------------------------------|
| `arc.session.attach`       | `{session_id, token}`                     | join mirrored session         |
| `arc.session.control`      | `{session_id, requester_id}`              | claim write control           |
| `arc.session.release`      | `{session_id}`                            | release write control         |
| `arc.ai.chat`              | `{model, messages[]}`                     | P3 ArcProvider deferred route |
| `arc.ai.stream_chat`       | `{model, messages[]}`                     | streaming variant             |
| `arc.ai.list_models`       | `{}`                                      | enumerate available models    |

### Notifications

| Notification name           | Params shape                              | Direction     |
|-----------------------------|-------------------------------------------|---------------|
| `arc.mirror.frame`          | `{session_id, data: bytes_b64}`           | relay→watcher |
| `arc.mirror.control_grant`  | `{session_id, holder_id}`                 | relay→all     |
| `arc.mirror.control_revoke` | `{session_id}`                            | relay→all     |
| `arc.session.participant`   | `{session_id, user_id, joined: bool}`     | relay→all     |

Reference: `arc-relay/docs/ai-route.md` (P3 deferred endpoint spec).

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
| `remote.exec`              | `{remote_session_id, command}`           | run one-off command               | advertised, returns server_error "not yet implemented in jsonrpc spine" — needs russh exec wiring |
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
5. `arc.*` — dependent on arc-relay WebSocket refactor
6. `remote.*` — connect/disconnect + 4 notifs **DONE** (branch `feat/jsonrpc-remote`); exec/file.read/file.write/forward.start advertised but stubbed pending russh wiring

Each migration step should remain feature-flagged (`FeatureFlag::JsonRpcSpine`) until
the full boundary is cut over, then the flag can be removed.
