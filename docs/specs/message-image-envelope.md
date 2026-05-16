# MessageImage envelope — spec

**Status:** draft v0.1 (2026-05-16, beam-claude).
**Scope:** wire shape for inline image content inside `arc.ai.stream_chat` and `arc.ai.chat` messages.
**Source-of-truth implementation:** WarpMod PR #111 (`app/src/ai/agent_loop/loop_runner.rs:108-128`).
**Closes:** REQUIREMENTS.md §2 "Multimodal images" convergence row, which named "Beam's MessageImage" as the canonical shape without defining it.

---

## Why this spec exists

Both clients (Beam, Beacon) and the relay (`arc-relay`) need a shared envelope for inline image payloads on:

- **`arc.ai.stream_chat`** / **`arc.ai.chat`** — when a user message includes a screenshot, attached image, or pasted clipboard image.
- **`ai.tool_use` → tool result** — when a tool (notably `bb-tool-bridge`'s `ComputerUse` dispatcher) returns images alongside text. The relay re-emits these as part of the next user-multimodal turn that gets fed back to the model.

Per-provider on-the-wire formats differ substantially (Anthropic base64 + media_type, OpenAI data-URL, Gemini inline_data, Ollama raw base64 array). The envelope is the **provider-agnostic interchange shape** that the client preserves end-to-end and that each provider encoder transforms into the model-specific wire format at the egress boundary.

---

## Envelope shape

JSON object. All fields except `data` and `mime` are optional with the noted defaults.

```jsonc
{
  "data": "<base64-encoded image bytes>",   // REQUIRED. Standard base64, no data-URL prefix.
  "mime": "image/png",                       // REQUIRED. Lower-case MIME type. See "Allowed MIME types" below.
  "width": 1024,                             // OPTIONAL. Pixel width, integer. Informational; some providers use it.
  "height": 768,                             // OPTIONAL. Pixel height, integer. Informational; some providers use it.
  "alt": "Browser screenshot showing ..."    // OPTIONAL. Plain-text alternative description for accessibility / logs.
                                              //   Reserved; future providers may surface this to the model. Today
                                              //   it's stored client-side for log lines and not transmitted.
}
```

Field rules:

| Field | Type | Required | Notes |
| --- | --- | :---: | --- |
| `data` | string | ✅ | Standard base64 (RFC 4648 §4). No `data:` prefix. No URL-safe encoding (`-`/`_`). Decoded bytes must match `mime`. |
| `mime` | string | ✅ | Lower-case. See allowlist. Mismatch with `data` is undefined behavior (relay may reject). |
| `width` | integer | ⬜ | Pixels. Omit if unknown. Servers that need it (cost-tracking heuristics) treat missing as "unknown" and may compute from the decoded bytes. |
| `height` | integer | ⬜ | Pixels. Same rule as `width`. |
| `alt` | string | ⬜ | Max 1024 characters. Reserved for future model surfacing; today it's metadata only. |

### Allowed MIME types

| MIME | Wire support |
| --- | --- |
| `image/png` | Anthropic ✅, OpenAI ✅, Gemini ✅, Ollama ✅, Local ✅ |
| `image/jpeg` | Anthropic ✅, OpenAI ✅, Gemini ✅, Ollama ✅, Local ✅ |
| `image/webp` | Anthropic ✅, OpenAI ✅, Gemini ✅, Ollama ⚠️ (encoded but server-side support varies), Local ⚠️ |
| `image/gif` | Anthropic ✅ (first frame), OpenAI ✅ (first frame), Gemini ⚠️, Ollama ❌, Local ❌ |

Clients SHOULD prefer `image/png` for screenshots (lossless) and `image/jpeg` for camera-origin content (smaller). The relay does NOT re-encode; it transmits the bytes as-received to whichever provider the request targets.

### Recommended size limits

These are advisory; the relay enforces a hard **8 MiB** cap on a single `data` field (post-base64-decode) at ingress. Clients SHOULD downscale before sending if larger.

| Use case | Recommended max | Hard limit |
| --- | --- | --- |
| Computer-use screenshot (full screen) | 2048×2048, ≤2 MiB JPEG | 8 MiB |
| Chat-attached photo / paste | 4096×4096, ≤4 MiB | 8 MiB |
| Diagram / chart | 2048×2048, ≤1 MiB PNG | 8 MiB |

Rationale: Anthropic, OpenAI, and Gemini each have their own per-image caps (currently 5 MiB, 20 MiB, 7 MiB respectively at time of writing); the 8 MiB ingress cap is the conservative intersection plus headroom for base64 expansion (~33%).

---

## Where it sits in `arc.ai.stream_chat`

The `messages` array carries multimodal user turns by adding an `images` field:

```jsonc
{
  "method": "arc.ai.stream_chat",
  "params": {
    "model": "claude-sonnet-4-5",
    "messages": [
      {
        "role": "system",
        "content": "You are a helpful assistant."
      },
      {
        "role": "user",
        "content": "What's in this screenshot?",
        "images": [
          {
            "data": "iVBORw0KGgoAAAANSU...",
            "mime": "image/png",
            "width": 1024,
            "height": 768
          }
        ]
      }
    ],
    "tools": [/* ... */],
    "temperature": 0.7,
    "max_tokens": 4096
  }
}
```

Rules:

1. **`images` is only valid on `role: "user"` messages** today. `role: "assistant"` messages MUST NOT include `images` (no client surface generates assistant-authored images, and the upstream models reject it).
2. **`content` MAY be empty string** when an image is the entire payload. Some clients (e.g. drag-image-only paste) send `content: ""` plus a one-image array.
3. **Multiple images allowed.** The relay forwards them in array order. Each provider applies its own per-request limit (Anthropic: 20, OpenAI: 50, Gemini: 16, Ollama: provider-dependent).
4. **Order matters for tool-result attachments.** When a tool returns images (see next section), the synthesized follow-up user turn places images AFTER the text in the same `messages` entry; some providers (OpenAI) require text-first ordering.

---

## Tool-result attachment path

When a tool invocation returns images (typically `ComputerUse`'s screenshot capture), the agent loop synthesizes a follow-up user-multimodal message rather than embedding images in the `tool` role (the OpenAI Chat Completions API forbids images on `role: "tool"`; the workaround is canonical across providers for consistency).

```text
Round 1:  user → "Open the browser and navigate to example.com"
Round 2:  assistant → tool_call: ComputerUse{action: "screenshot"}
Round 3:  tool result → {content: "Screenshot captured (1024x768)", images: [<MessageImage>]}
Round 4:  synthesized user multimodal → "Screenshot captured (1024x768)", images: [<MessageImage>]
Round 5:  assistant → text response analyzing the screenshot
```

The synthesis happens client-side in `bb-tool-bridge` / `app/src/ai/agent_loop/computer_use_dispatcher.rs` (cite: WarpMod commit history for PR #111). The relay sees the synthesized user message as just another `arc.ai.stream_chat` `messages` entry — no special handling.

---

## Per-provider on-the-wire encoding

The relay's `arc.ai.stream_chat` handler routes to the named provider and applies the appropriate encoder. Clients never see provider-specific shapes directly.

### Anthropic Messages API

```jsonc
{
  "role": "user",
  "content": [
    { "type": "text", "text": "What's in this screenshot?" },
    {
      "type": "image",
      "source": {
        "type": "base64",
        "media_type": "image/png",
        "data": "iVBORw0KGgoAAAANSU..."
      }
    }
  ]
}
```

Encoder: `app/src/ai/agent_loop/providers/anthropic.rs:280` (handles `UserMultimodal { text, images }`).

### OpenAI Chat Completions (and OpenAI-compat: OpenRouter, Together, Groq, Moonshot, Cerebras, Anyscale)

```jsonc
{
  "role": "user",
  "content": [
    { "type": "text", "text": "What's in this screenshot?" },
    {
      "type": "image_url",
      "image_url": {
        "url": "data:image/png;base64,iVBORw0KGgoAAAANSU..."
      }
    }
  ]
}
```

Encoder: `app/src/ai/agent_loop/providers/openai_compat.rs:203` (handles `UserMultimodal` and the tool-result synthesis path).

### Gemini

```jsonc
{
  "role": "user",
  "parts": [
    { "text": "What's in this screenshot?" },
    {
      "inline_data": {
        "mime_type": "image/png",
        "data": "iVBORw0KGgoAAAANSU..."
      }
    }
  ]
}
```

Encoder: `app/src/ai/agent_loop/providers/gemini.rs:187`.

### Ollama (native chat API)

```jsonc
{
  "role": "user",
  "content": "What's in this screenshot?",
  "images": [
    "iVBORw0KGgoAAAANSU..."
  ]
}
```

Encoder: `app/src/ai/agent_loop/providers/ollama.rs:75`. Ollama's API is unusual in that `images` is a parallel string array of raw base64, not interleaved with text content.

### Local OpenAI-compat (Ollama-on-`/v1`, LM Studio, vLLM, llama.cpp)

Same wire shape as OpenAI Chat Completions. The local servers either implement the `image_url` data-URL form natively or transparently transcode.

---

## Streaming semantics

`arc.ai.stream_chat` returns notifications (`ai.chunk`, `ai.done`, `ai.error`, `ai.tool_use`). Images are NOT echoed in stream notifications — the model's response is text-only on the streaming side. The envelope only flows on the inbound `messages` array.

If a future provider supports streamed image generation (e.g. inline-generated diagrams), this spec needs a `MessageImage`-out variant on `ai.chunk`. Out of scope for v0.

---

## Decoding obligations on the relay

The `arc-relay` ingress validator MUST:

1. **Cap each `data` field at 8 MiB** post-base64-decode. Reject with JSON-RPC `-32602` (`invalid_params`) on overflow.
2. **Reject mismatched `mime`/`data`** by inspecting the magic-number prefix of the decoded bytes (PNG `89 50 4E 47`, JPEG `FF D8 FF`, WebP `52 49 46 46 ... 57 45 42 50`). Mismatch → `-32602`.
3. **Forward `width`/`height` as-given** without recomputing. Clients are trusted for these; relay-side cost metering uses them as-is.
4. **Re-emit `alt` to provider only if provider supports it.** Today this is a no-op for all listed providers.

---

## Versioning

This envelope is v0.1 — initial draft, subject to revision before any client ships against ARC's image surface in production. Breaking changes between v0.x revisions are permitted; once a `MessageImageEnvelopeV1` semver pin is declared in `arc-relay`'s `Cargo.toml`, breaking changes require a new spec version + relay-side dual-shape support during a deprecation window per `docs/tier-one-plan.md` T1.1.

---

## Out of scope for v0

- **Video / animated image content.** GIF is supported but first-frame-only on most providers.
- **Image generation (text→image)** as a relay-emitted artifact.
- **Image URLs hosted on R2 / external storage.** Today all images are inline-base64. A future revision may add `{kind: "url", url: "https://r2.../..."}` as a sibling of `{kind: "inline", data, mime}` to save bandwidth on large images, but that requires the relay to act as a URL fetcher/proxy.
- **Per-image cost attribution.** Cost tracking happens at the message level; per-image pricing is a future enhancement.

---

## References

- WarpMod PR #111 — initial `MessageImage` + `ProviderMessage::UserMultimodal` implementation.
- WarpMod `app/src/ai/agent_loop/loop_runner.rs:108-128` — `MessageImage` struct definition.
- WarpMod `app/src/ai/agent_loop/computer_use_dispatcher.rs` — tool-result image attachment path.
- Anthropic [Messages API: Vision](https://docs.anthropic.com/en/docs/build-with-claude/vision).
- OpenAI [Chat Completions: image_url](https://platform.openai.com/docs/guides/vision).
- Google [Gemini multimodal](https://ai.google.dev/gemini-api/docs/vision).
- Ollama [Multimodal models](https://github.com/ollama/ollama/blob/main/docs/api.md#parameters-1).

---

**Edit log**

| Date | By | Change |
| --- | --- | --- |
| 2026-05-16 | beam-claude | Initial draft. Envelope shape + per-provider encodings + decoding obligations. Deliverable B2 from arc-009. |
