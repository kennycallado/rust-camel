# Proposal: llm-response-body-cap

## Why

Mission 555 (`f862b0f2b`, bd `rc-9pfby`) removed the client-wide 30 s total
timeout and replaced it with `LlmHttpPolicy` (connect 10 s + 600 s read-idle
guard on chat/embed). The read-idle guard bounds *inactivity*, not *size*: a
peer that keeps sending bytes grows the accepted response payload without a
ceiling. Without a producer deadline (`timeout_secs: None`), such a
connection persists indefinitely. Three sites where Camel accepts unbounded
payloads:

- **Materialized chat** — `full_text` accumulated in `producer.rs` inside the
  stream loop that feeds `ChatEvent::Delta`.
- **Chat stream** — bytes forwarded as deltas from genai chunks in
  `genai_adapter.rs`.
- **Embed** — decoded vectors converted into `EmbedResponse` from the genai
  response.

The decision path already has the model: `read_capped` +
`MAX_DECISION_RESPONSE_BYTES` (8 MiB) in `decision/http.rs` — that path is
not touched.

Closes bd `rc-kemrg` (P2, discovered-from `rc-9pfby`).

## What Changes

- New `LlmError::ResponseTooLarge { path, cap_bytes, read_bytes }` — typed,
  names the path, the cap, and the attempted cumulative counted bytes;
  **non-retryable**: retrying re-requests a payload that breaches the cap
  again — retries amplify load with no path to success, so the budget is
  enforced once, at the read boundary.
- `MAX_CHAT_RESPONSE_BYTES` (8 MiB) and `MAX_EMBED_RESPONSE_BYTES` (64 MiB):
  explicit fixed safety budgets on *accepted payloads* — UTF-8 text bytes
  (chat) or f32 vector bytes (embed). Enforced while reading/forwarding on
  every path where Camel holds the bytes: genai-backed streaming (all
  providers through the genai adapter) and materialized accumulation (all
  providers, mock included); enforced at the Camel conversion boundary for
  embed (genai reads the embed JSON internally, before Camel sees data).
  Direct streaming from non-genai providers (mock/custom adapters) is not
  capped — recorded residual. Inclusive limit: exactly the cap is accepted;
  cap + 1 byte is rejected.
- **These caps bound accepted payloads, not wire bodies or process memory.**
  genai SDK internals stay unbounded (SSE buffering, an oversized individual
  stream event, captured tool arguments inside `StreamEnd`, the embed JSON
  body, genai error bodies). Camel-side error text stays truncated to 200
  bytes as today. ADR-0021 addendum updated with the narrowed residual and a
  follow-up bd issue filed.
- Semantics: responses at or below the cap are byte-identical. A breach is an
  error, never silent truncation. Streaming deltas delivered before the
  breach stay delivered; the breaching chunk is not forwarded; the stream
  terminates with the cap error and no `Finished` event. Provider-stream
  consumers observe `LlmError`; producer/exchange consumers observe
  `CamelError` (mapped through the existing `From<LlmError> for CamelError`).
- Spec delta on `llm-provider-adapter` (owner of transport bounds since 555).

## Acceptance criteria

- Under-cap staged responses complete byte-identically (chat, embed);
  exactly-cap is accepted; cap + 1 is rejected.
- Over-cap calls fail with `ResponseTooLarge` naming path, cap, and counted
  bytes; no Camel-owned buffering past the cap.
- Streaming: deltas before the breach were delivered; terminal error is the
  cap error; an oversized first delta forwards nothing.
- Multibyte UTF-8 text is counted by bytes, not chars.
- `camel-component-llm` lib suites green with `--features openai` and
  `--features decisions`; `scripts/gates-local.sh` exit 0.
- Decision path, `read_capped`, `MAX_DECISION_RESPONSE_BYTES`, timeout
  policy values, retry/backoff, SSRF/pinning, lazy client build: unchanged.

## Risk budget

- Caps are generous constants (not configurable); they are safety budgets,
  not guarantees that every wire body fits. Config knobs are out of bounds
  for this change.
- No new dependency; no change to transport policy values or the decision
  path. SDK-internal buffers are accepted residual, documented in ADR-0021
  with a follow-up bd issue.
