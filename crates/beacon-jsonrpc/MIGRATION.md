# beacon-jsonrpc — Remaining migration targets

This file documents every cross-process boundary that should eventually migrate
to `beacon-jsonrpc` as part of the M3 "transport unification" milestone.

---

## 1. hostd ↔ Mac client  (currently: uniffi events over dylib FFI)

Source files: `beacon-core/crates/beacon-ffi/src/lib.rs`,
`beacon-core/crates/beacon-hostd/src/lib.rs`,
`beacon-mac/Sources/BeaconMac/Bridge/`.

### Methods (request/response)

| Method name             | Params shape                              | Result shape                  | Source fn / type          |
|-------------------------|-------------------------------------------|-------------------------------|---------------------------|
| `session.create`        | `{workspace_id, shell, cwd}`              | `{session_id}`                | `FfiHostd::create_session`|
| `session.attach`        | `{session_id}`                            | `{ok: bool}`                  | `FfiHostd::attach`        |
| `session.resize`        | `{session_id, cols, rows}`               | `{ok: bool}`                  | `FfiHostd::resize`        |
| `session.write`         | `{session_id, data: string}`              | `{ok: bool}`                  | `FfiHostd::write`         |
| `session.close`         | `{session_id}`                            | `{ok: bool}`                  | `FfiHostd::close_session` |
| `memory.search`         | `{query, top_k, workspace_id?}`           | `[{text, score}]`             | `FfiMemoryStore::search`  |
| `memory.upsert`         | `{entry: MemoryEntry}`                    | `{id: string}`                | `FfiMemoryStore::upsert`  |
| `ai.send_message`       | `{session_id, message}`                   | streaming via notifications   | `FfiAiSession::send_message` |
| `command.describe`      | `{command: string}`                       | `{description: string}`       | **DONE** — see beacon-command/src/rpc.rs |

### Notifications (server → client, no response)

| Notification name        | Params shape                              | Emitted when                  |
|--------------------------|-------------------------------------------|-------------------------------|
| `session.output`         | `{session_id, data: string}`              | PTY produces bytes            |
| `session.title_changed`  | `{session_id, title: string}`             | OSC 0/2 sequence              |
| `session.cwd_changed`    | `{session_id, cwd: string}`               | OSC 7 sequence                |
| `session.block_end`      | `{session_id, block: Block}`              | OSC 133;D closes a block      |
| `ai.chunk`               | `{session_id, delta: string}`             | Streaming AI token            |
| `ai.done`                | `{session_id, usage?}`                    | AI response complete          |
| `ai.error`               | `{session_id, error: string}`             | AI call failed                |

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

1. `command.describe` — **DONE** (this PR, gated by `FeatureFlag::JsonRpcSpine`)
2. `session.*` + `session.*` notifications — highest-frequency; unblocks live mirror
3. `ai.*` — needed for streaming tokens over WebSocket
4. `arc.*` — dependent on arc-relay WebSocket refactor
5. `remote.*` — can run in parallel with arc.* once P6 SSH is fleshed out

Each migration step should remain feature-flagged (`FeatureFlag::JsonRpcSpine`) until
the full boundary is cut over, then the flag can be removed.
