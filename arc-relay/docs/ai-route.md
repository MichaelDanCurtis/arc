# Arc Relay — AI Route Contract

This document specifies the four AI endpoints that the Arc relay **must** implement
so that the Beacon Mac client's `ArcProvider` can route AI requests through it.

`ArcProvider` source: `beacon-core/crates/beacon-ai-core/src/providers/arc.rs`

---

## Authentication

Every request must carry a valid session token:

```
Authorization: Bearer <session_token>
```

The token is issued by Arc's OAuth/login flow and stored by the Mac client in
`UserDefaults` under `arc.session_token`. Requests with missing or invalid tokens
MUST return **401 Unauthorized** with an empty body.

---

## Endpoints

### POST `/v1/ai/chat`

Non-streaming chat request.

**Request body** (JSON):

```json
{
  "provider": "anthropic",
  "model": "claude-opus-4-5",
  "messages": [
    { "role": "user", "content": "Hello" }
  ],
  "tools": [
    {
      "type": "function",
      "function": {
        "name": "run_command",
        "description": "Run a shell command",
        "parameters": { "type": "object", "properties": { "command": { "type": "string" } } }
      }
    }
  ],
  "max_tokens": 1024,
  "temperature": 0.7,
  "system": "You are Beacon AI."
}
```

Fields:
- `provider` — upstream provider name (e.g. `"anthropic"`, `"openai"`, `"google"`). The relay uses this to route to the correct upstream using its server-side credentials. May be empty string to use relay default.
- `model` — bare model id (e.g. `"claude-opus-4-5"`) without provider prefix.
- `messages` — OpenAI-compat message array. Roles: `"system"`, `"user"`, `"assistant"`, `"tool"`. Assistant turns with tool calls carry a `tool_calls` array (same shape as OpenAI). Tool result turns carry `tool_call_id`.
- `tools` — optional array of function tool definitions (OpenAI function-calling format).
- `max_tokens`, `temperature`, `system` — optional upstream hints; relay may ignore if upstream does not support.

**Response** (200 OK, JSON) — OpenAI-compat chat completion:

```json
{
  "choices": [
    {
      "message": {
        "content": "The directory is empty.",
        "tool_calls": []
      }
    }
  ],
  "model": "anthropic:claude-opus-4-5",
  "usage": {
    "prompt_tokens": 42,
    "completion_tokens": 8
  }
}
```

If the model requests tool calls instead of producing text, `message.content` should be `null` and `message.tool_calls` non-empty:

```json
{
  "choices": [
    {
      "message": {
        "content": null,
        "tool_calls": [
          {
            "id": "call_abc123",
            "function": {
              "name": "run_command",
              "arguments": "{\"command\":\"ls /tmp\"}"
            }
          }
        ]
      }
    }
  ],
  "model": "anthropic:claude-opus-4-5",
  "usage": { "prompt_tokens": 20, "completion_tokens": 8 }
}
```

---

### POST `/v1/ai/stream_chat`

Streaming chat. Same request body as `/v1/ai/chat`.

**Response** — `Content-Type: text/event-stream`, OpenAI-compat SSE delta stream:

```
data: {"choices":[{"delta":{"content":"Hello"},"finish_reason":null}]}

data: {"choices":[{"delta":{"content":" world"},"finish_reason":null}]}

data: {"choices":[{"delta":{"content":"!"},"finish_reason":"stop"}]}

data: [DONE]
```

Tool call streaming — deltas carry `delta.tool_calls` indexed by `index`:

```
data: {"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_abc","function":{"name":"run_command","arguments":""}}]},"finish_reason":null}]}

data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"command\":"}}]},"finish_reason":null}]}

data: {"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"ls /tmp\"}"}}]},"finish_reason":"tool_calls"}]}

data: [DONE]
```

When `finish_reason` is `"tool_calls"` the `ArcProvider` emits a `StreamEvent::ToolUse` for each accumulated call.

The relay MUST terminate every stream with `data: [DONE]`.

---

### POST `/v1/ai/complete`

Single-shot ghost-text completion (short, low-latency).

**Request body** (JSON):

```json
{
  "model": "anthropic:claude-haiku-4-5",
  "prompt": "git log ",
  "max_tokens": 32
}
```

**Response** (200 OK, JSON):

```json
{
  "text": "--oneline --graph"
}
```

Only the `text` field is required. The relay may add token counts or other metadata but they are ignored by the client.

---

### GET `/v1/ai/list_models`

Returns all models available through this relay.

**Response** (200 OK, JSON):

```json
{
  "models": [
    {
      "id": "anthropic:claude-opus-4-5",
      "name": "Claude Opus 4.5",
      "context_window": 200000
    },
    {
      "id": "openai:gpt-4o",
      "name": "GPT-4o",
      "context_window": 128000
    },
    {
      "id": "google:gemini-2.0-flash"
    }
  ]
}
```

Fields per model:
- `id` — required; `"provider:model"` composite used by the client.
- `name` — optional display name; falls back to `id` if absent.
- `context_window` — optional integer token count.

---

## Error responses

| Status | Meaning | ArcProvider maps to |
|--------|---------|---------------------|
| 401    | Token expired / invalid | `AiError::AuthFailed` |
| 403    | Token lacks permission | `AiError::AuthFailed` |
| 429    | Rate limited | `AiError::RateLimited` (retry_after=60s) |
| 5xx    | Relay or upstream error | `AiError::Provider` |

The relay may include a JSON body with an `"error"` or `"message"` field; the Mac
client does not parse it — only the HTTP status code matters.

---

## Implementation notes for arc-relay sub-agent

1. **Route table** — add four routes in `arc-relay/src/api/`:
   - `POST /v1/ai/chat` → handler fans out to upstream provider SDK
   - `POST /v1/ai/stream_chat` → streams SSE; flush after each delta
   - `POST /v1/ai/complete` → single upstream call, return `{text}`
   - `GET /v1/ai/list_models` → merge lists from configured providers

2. **Auth middleware** — reuse the existing JWT/session middleware from
   `arc-relay/src/auth/`. The `session_token` is the same token used for relay sessions.

3. **Provider fan-out** — use server-side API keys (Anthropic, OpenAI, Google) stored in
   relay environment variables. The `provider` field in the request body selects the
   upstream. Unknown provider → 422 Unprocessable Entity.

4. **No state** — these are stateless HTTP endpoints. The relay does NOT persist chat
   history; the Mac client owns the conversation history and sends the full message
   array on every request.

5. **Streaming flush** — for `/v1/ai/stream_chat`, ensure each SSE `data:` line is
   flushed immediately (disable response buffering). The Mac SSE parser expects line-by-line delivery.

6. **CORS** — not required (Mac native client, no browser).
