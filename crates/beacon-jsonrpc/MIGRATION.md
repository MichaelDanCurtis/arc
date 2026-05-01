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
| `ai.send_message`       | `{session_id, message}`                   | streaming via notifications   | `FfiAiSession::send_message` | still uniffi — deferred to follow-up |
| `command.describe`      | `{command: string}`                       | `{description: string}`       | **DONE** — see beacon-command/src/rpc.rs | **DONE** |

### Notifications (server → client, no response)

| Notification name        | Params shape                              | Emitted when                  | Status |
|--------------------------|-------------------------------------------|-------------------------------|--------|
| `session.output`         | `{session_id, data: string}`              | PTY produces bytes            | **DONE** — `emit_output()` in beacon-hostd/src/rpc.rs |
| `session.title_changed`  | `{session_id, title: string}`             | OSC 0/2 sequence              | **DONE** — `emit_title_changed()` in beacon-hostd/src/rpc.rs |
| `session.cwd_changed`    | `{session_id, cwd: string}`               | OSC 7 sequence                | **DONE** — `emit_cwd_changed()` in beacon-hostd/src/rpc.rs |
| `session.block_end`      | `{session_id, block: Block}`              | OSC 133;D closes a block      | **DONE** — `emit_block_end()` in beacon-hostd/src/rpc.rs |
| `ai.chunk`               | `{session_id, delta: string}`             | Streaming AI token            | still uniffi — deferred to follow-up |
| `ai.done`                | `{session_id, usage?}`                    | AI response complete          | still uniffi — deferred to follow-up |
| `ai.error`               | `{session_id, error: string}`             | AI call failed                | still uniffi — deferred to follow-up |

### Notification channel design (feat/jsonrpc-hostd)

Notifications are delivered via `tokio::sync::broadcast::Sender<String>` held inside
each `SessionEntry` in `beacon-hostd::rpc::SessionStore`. Each notification string is
a fully-serialised JSON-RPC 2.0 `Notification` object.

Callers subscribe by calling `FfiHostdRpc::subscribe_notifications(session_id)` which
returns an `Arc<FfiNotifReceiver>`. They poll it with `FfiNotifReceiver::poll_next()`.
This polling approach was chosen over FFI callbacks to avoid the cross-thread
complexity of callback interfaces when the notification may fire from any async task.

### ai.* status (feat/jsonrpc-hostd)

`ai.send_message`, `ai.chunk`, `ai.done`, `ai.error` remain on the uniffi surface.
Migrating them requires streaming-over-RPC design which is deferred to follow-up
branches.

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

## 3. beacon-remote  (SSH transport, currently stub)

Source files: `beacon-core/crates/beacon-remote/src/lib.rs`.

Branch: `feat/beacon-remote` (P6 stub, a376d77).

### Methods

| Method name                | Params shape                              | Notes                             |
|----------------------------|-------------------------------------------|-----------------------------------|
| `remote.connect`           | `{host, port, user, auth}`               | initiate SSH session              |
| `remote.disconnect`        | `{remote_session_id}`                    | clean shutdown                    |
| `remote.exec`              | `{remote_session_id, command}`           | run one-off command               |
| `remote.file.read`         | `{remote_session_id, path}`              | SFTP read                         |
| `remote.file.write`        | `{remote_session_id, path, content_b64}` | SFTP write                        |
| `remote.forward.start`     | `{remote_session_id, local_port, remote_port}` | port forward (deferred)   |

### Notifications

| Notification name          | Params shape                              | Direction         |
|----------------------------|-------------------------------------------|-------------------|
| `remote.output`            | `{remote_session_id, data}`              | daemon→client     |
| `remote.disconnected`      | `{remote_session_id, reason}`            | daemon→client     |

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
4. `ai.*` (`ai.send_message` + streaming notifications) — still uniffi; requires streaming-over-RPC design
5. `arc.*` — dependent on arc-relay WebSocket refactor
6. `remote.*` — can run in parallel with arc.* once P6 SSH is fleshed out

Each migration step should remain feature-flagged (`FeatureFlag::JsonRpcSpine`) until
the full boundary is cut over, then the flag can be removed.
