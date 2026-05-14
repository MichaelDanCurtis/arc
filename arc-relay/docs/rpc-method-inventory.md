# ARC Relay JSON-RPC Method Inventory

**Generated:** 2026-05-14  
**Scope:** Complete enumeration of every JSON-RPC method and notification the Arc relay serves, mapped to REQUIREMENTS.md sections.

**Legend:**
- **Method**: Client-initiated RPC call with response (has `fn method_name()` handler)
- **Notification**: Server-initiated broadcast, no response expected
- **Beacon-only / Beam-only / Both**: Target client surface(s)

---

## Methods (RPC call handlers)

| Method Name | Type | REQUIREMENTS.md Section | Source File:Line | One-line Purpose | Transport |
|---|---|---|---|---|---|
| `arc.session.attach` | method | §3 Session mirroring | rpc.rs:549 | Join a mirrored session as a watcher | Both |
| `arc.session.detach` | method | §3 Session mirroring | rpc.rs:600 | Leave a mirrored session | Both |
| `arc.session.subscribe` | method | §3 Session mirroring | rpc.rs:655 | Begin subscribing to session output (read-only handshake) | Both |
| `arc.session.unsubscribe` | method | §3 Session mirroring | rpc.rs:707 | Stop subscribing to session output | Both |
| `arc.session.control` | method | §3 Session mirroring | rpc.rs:748 | Request write control over a session | Both |
| `arc.session.release` | method | §3 Session mirroring | rpc.rs:789 | Voluntarily release write control | Both |
| `arc.session.approve_control` | method | §3 Session mirroring | rpc.rs:836 | Approve a pending write-control request (holder only) | Both |
| `arc.session.deny_control` | method | §3 Session mirroring | rpc.rs:891 | Deny a pending write-control request (holder only) | Both |
| `arc.session.input` | method | §3 Session mirroring | rpc.rs:940 | Send PTY input bytes to the session (controller only) | Both |
| `arc.session.resize` | method | §3 Session mirroring | rpc.rs:991 | Resize the session PTY (controller only) | Both |
| `arc.host.hello` | method | §8 Beam-required endpoints | rpc.rs:1043 | Host registration heartbeat; negotiate protocol version | Beacon |
| `arc.host.session_register` | method | §8 Beam-required endpoints | rpc.rs:1124 | Host announces a new session; relay wires up mirroring | Beacon |
| `arc.host.session_unregister` | method | §8 Beam-required endpoints | rpc.rs:1363 | Host deregisters a session; relay cleans up mirroring | Beacon |
| `arc.host.block_start` | method | §8 Beam-required endpoints | rpc.rs:1466 | Host notifies relay of new command block; relay broadcasts to watchers | Beacon |
| `arc.host.block_chunk` | method | §8 Beam-required endpoints | rpc.rs:1513 | Host sends output chunk for a block; relay fans to watchers | Beacon |
| `arc.host.block_end` | method | §8 Beam-required endpoints | rpc.rs:1563 | Host signals block completion with exit code; relay broadcasts | Beacon |
| `arc.host.session_resize` | method | §8 Beam-required endpoints | rpc.rs:1615 | Host notifies relay of PTY resize; relay updates state | Beacon |
| `arc.host.push_output` | method | §8 Beam-required endpoints | rpc.rs:1674 | Host sends live session output; relay broadcasts to watchers | Beacon |
| `arc.host.shadow_diff_response` | method | §8 Beam-required endpoints | rpc.rs:1771 | Host replies to relay's shadow-diff request | Beacon |
| `arc.ai.chat` | method | §2 AI chat streaming | ai_rpc.rs:388 | Non-streaming AI chat; blocks until response assembled | Both |
| `arc.ai.stream_chat` | method | §2 AI chat streaming | ai_rpc.rs:424 | Streaming AI chat; spawns task, returns immediately; emits ai.chunk/done/error | Both |
| `arc.ai.list_models` | method | §2 AI chat streaming | ai_rpc.rs:506 | Enumerate available AI models; returns {models:[...]} | Both |

---

## Notifications (broadcast-only, no RPC response)

| Notification Name | Type | REQUIREMENTS.md Section | Source File:Line | One-line Purpose | Transport |
|---|---|---|---|---|---|
| `arc.session.participant` | notification | §3 Session mirroring | rpc.rs:171 | Relay→all; user joined/left session | Both |
| `arc.session.control_requested` | notification | §3 Session mirroring | rpc.rs:427 | Relay→holder; incoming write-control request from watcher | Both |
| `arc.session.gone` | notification | §3 Session mirroring | rpc.rs:442 | Relay→all; host process disconnected; session terminated | Both |
| `arc.session.revoked` | notification | §3 Session mirroring | rpc.rs:456 | Relay→watcher; session owner kicked this watcher out | Both |
| `arc.mirror.frame` | notification | §3 Session mirroring | rpc.rs:189 | Relay→watchers; live PTY output from host; base64-encoded | Both |
| `arc.mirror.control_grant` | notification | §3 Session mirroring | rpc.rs:203 | Relay→all; write control transferred to new holder | Both |
| `arc.mirror.control_revoke` | notification | §3 Session mirroring | rpc.rs:215 | Relay→all; write control released, no successor | Both |
| `arc.mirror.discontinuity` | notification | §3 Session mirroring | rpc.rs:227 | Relay→watcher; broadcast buffer overflow; gap signal | Both |
| `arc.mirror.block_start` | notification | §3 Session mirroring | rpc.rs:246 | Relay→watchers; new command block begins; full BlockHeader included | Both |
| `arc.mirror.block_chunk` | notification | §3 Session mirroring | rpc.rs:263 | Relay→watchers; block output chunk with seq for gap detection | Both |
| `arc.mirror.block_end` | notification | §3 Session mirroring | rpc.rs:286 | Relay→watchers; command block finished; full BlockFooter included | Both |
| `arc.mirror.replay_begin` | notification | §3 Session mirroring | rpc.rs:304 | Relay→watcher; scrollback replay starting; total_blocks count | Both |
| `arc.mirror.replay_block` | notification | §3 Session mirroring | rpc.rs:321 | Relay→watcher; one historical block during replay | Both |
| `arc.mirror.replay_end` | notification | §3 Session mirroring | rpc.rs:345 | Relay→watcher; scrollback replay finished; live frames follow | Both |
| `arc.host.input` | notification | §3 Session mirroring | rpc.rs:357 | Relay→host; PTY bytes from current driver; forwarded by host-side consumer | Beacon |
| `arc.mirror.host_input` | notification | §3 Session mirroring | rpc.rs:375 | Relay→all-watchers; echo of current driver's input, attributed via from_user | Both |
| `arc.host.resize_request` | notification | §3 Session mirroring | rpc.rs:389 | Relay→host; PTY resize request from driver; forwarded by host-side consumer | Beacon |
| `arc.host.shadow_diff_request` | notification | §3 Session mirroring | rpc.rs:406 | Relay→host; request for shadow-diff snapshot; pairs with host's response | Beacon |
| `ai.chunk` | notification | §2 AI chat streaming | ai_rpc.rs:263 | Relay→subscribers; partial text delta from streaming chat | Both |
| `ai.done` | notification | §2 AI chat streaming | ai_rpc.rs:272 | Relay→subscribers; stream finished successfully; full assembled content | Both |
| `ai.error` | notification | §2 AI chat streaming | ai_rpc.rs:285 | Relay→subscribers; stream encountered error; stream terminates | Both |
| `ai.tool_use` | notification | §2 AI chat streaming | ai_rpc.rs:296 | Relay→subscribers; model requested tool invocation | Both |

---

## Gaps & Coverage Analysis

### 1. Methods Shipped but Not Yet Covered in REQUIREMENTS.md

The following are **fully implemented and shipping** but have no explicit row in REQUIREMENTS.md's capability table:

- **arc.session.approve_control**, **arc.session.deny_control** — Control approval workflow (§3)
- **arc.session.input**, **arc.session.resize** — Driver-only PTY mutation methods (§3)
- **arc.host.session_register**, **arc.host.session_unregister**, **arc.host.block_start**, **arc.host.block_chunk**, **arc.host.block_end**, **arc.host.session_resize**, **arc.host.push_output**, **arc.host.shadow_diff_response** — Complete host protocol surface (§8)

**Action:** These should be added to REQUIREMENTS.md as rows under "§3 Session mirroring + remote control" with Convergence column saying "Shipped".

---

### 2. REQUIREMENTS.md Sections With No Corresponding Shipped Method

| Section | Proposed Methods | Shipped? | Status |
|---|---|---|---|
| §1 User auth (OAuth + session token) | `/api/auth/oauth/<provider>`, `/api/auth/refresh` | ❌ | HTTP routes exist in auth_routes.rs but not in JSON-RPC surface; user auth is gate-level via bearer token |
| §4 User-to-user messaging (arc.msg.*) | `arc.msg.dm_send`, `arc.msg.channel_create`, etc. | ❌ | **Proposed but not shipped.** No RPC handlers; no broadcast channels |
| §5 Agent-to-Execution (arc.exec.*) | `arc.exec.spawn`, `arc.exec.target_self`, etc. | ❌ | **Proposed but not shipped.** No RPC handlers; referenced in REQUIREMENTS but unimplemented |
| §6 Plugin host | Plugin capability traits | ⚠️ | Path-dependency stub exists; gRPC contract not in JSON-RPC surface |
| §7 Marketplace | Browse, install, publish | ❌ | Out of scope; handled by external ailuxis marketplace service |
| §8 New endpoints Beam needs | `/api/current_time`, `/api/v1/telemetry`, `/api/v1/conversations/sync`, etc. | ❌ | Stub HTTP routes exist but not JSON-RPC; planned for future slices |

---

### 3. Method Count by REQUIREMENTS.md Section

| Section | Methods | Notifications | Total |
|---|---|---|---|
| §1 User auth | 0 | 0 | 0 |
| §2 AI chat streaming | 3 | 4 | 7 |
| §3 Session mirroring + remote control | 10 | 14 | 24 |
| §4 User-to-user messaging | 0 | 0 | 0 |
| §5 Agent-to-Execution | 0 | 0 | 0 |
| §6 Plugin host | 0 | 0 | 0 |
| §7 Marketplace | 0 | 0 | 0 |
| §8 Beam-required endpoints | 9 | 4 | 13 |
| **Total** | **22** | **22** | **44** |

---

## Wire Protocol Summary

**Shipped surface:** 22 JSON-RPC methods + 22 notifications (44 total).

**Transports:**
- **Beacon** (WarpClone client): All 44 methods/notifications
- **Beam** (WarpMod client): 44 methods/notifications (omitting host-side ones: `arc.host.*` methods and their notifications)

**Auth:** Bearer token (JWT) on WebSocket upgrade query + HTTP Authorization header; revalidated every 30s via mid-connection token decode.

**Encoding:** UTF-8 JSON with base64 for binary payloads (PTY frames, block output, etc.).

**Subprotocol:** `arc.json-rpc` per §9 Wire-protocol invariants.

---

## Out-of-Scope Surfaces

The following capabilities mentioned in REQUIREMENTS.md are **explicitly outside JSON-RPC**:

- User auth OAuth flow: HTTP `POST /api/auth/register`, `POST /api/auth/login`, `POST /api/auth/refresh`
- Plugin distribution: External ailuxis marketplace (not Arc)
- Payment: Handled by billing service (not Arc)
- File transfer: Arc stores session output; file sharing is future work

---

**Edit log**

| Date | Change |
|---|---|
| 2026-05-14 | Initial inventory: 22 methods, 22 notifications across §2, §3, §8. Identified 3 major gaps: (1) arc.msg.* unshipped, (2) arc.exec.* unshipped, (3) auth routes are HTTP, not JSON-RPC. |
