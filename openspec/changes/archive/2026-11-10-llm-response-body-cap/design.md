# Design: llm-response-body-cap

## Approach

Extend the decision path's philosophy (`read_capped` + `MAX_DECISION_RESPONSE_BYTES`)
to the chat/embed paths, adapted per site — while-reading enforcement wherever
Camel code holds the bytes, conversion-boundary enforcement where only genai
holds them.

**Guarantee scope (narrowed per spec blessing).** The caps bound *the payload
Camel accepts*: UTF-8 text bytes on chat paths, f32 vector bytes on embed.
They are explicit safety budgets, NOT wire-body or process-memory guarantees.
genai SDK internals remain unbounded: SSE line buffering, an oversized
individual stream event (the SDK accumulates before emitting a chunk),
captured tool-call arguments inside `StreamEnd`, the embed JSON body it reads
with `.text()`, and genai error bodies. Bounding those requires a genai fork
or a new dependency — both out of bounds for this change. Camel-side error
text stays truncated to 200 bytes via `LlmError::provider` / `map_status` as
today.

**Error type.** Add `LlmError::ResponseTooLarge { path: &'static str,
cap_bytes: usize, read_bytes: usize }` to the existing taxonomy
(`src/error.rs`). Display: `response body exceeds 8388608-byte cap on
chat_stream: counted 8389123 bytes` — names the path, the cap, and the
attempted cumulative counted bytes (including the breaching chunk).
Non-retryable: `is_retryable` is a whitelist; the new variant is absent from
it. Justification: retrying re-requests a payload that breaches the cap again
— retries amplify load with no path to success, so the budget is enforced
once at the read boundary.
`From<LlmError> for CamelError` maps it through the existing `ProcessorError`
catch-all — a volume fault is a processing failure, so producer/exchange
consumers observe `CamelError::ProcessorError` (including through the
returned streaming body), while direct provider-stream consumers observe
`LlmError`.

**Caps (constants, documented, not configurable).**

- `MAX_CHAT_RESPONSE_BYTES = 8 MiB` (`src/lib.rs`, next to `LlmHttpPolicy`):
  a fixed budget, same class as the decision path's 8 MiB. It bounds
  worst-case accepted payload memory per chat request at a level well above
  typical completions. It is a chosen trade-off — bounded accepted size over
  the remote chance of rejecting an extreme legitimate completion — not a
  claim that no wire body can exceed it.
- `MAX_EMBED_RESPONSE_BYTES = 64 MiB`: f32 vector bytes, a deliberate fixed
  budget. For scale: 64 MiB ≈ 5,461 × 3,072-dim (v3-large) vectors. Same
  trade-off framing: bounded accepted size; not a claim about batch limits.
- Inclusive limit: a payload of exactly `cap_bytes` is accepted; `cap_bytes
  + 1` is rejected. Counting: `str::len()` (UTF-8 bytes, never chars) for
  text; `v.len() * size_of::<f32>()` summed over vectors for embed.
  Accumulation uses overflow-safe arithmetic (`checked_add`); on overflow the
  cap is reported as the budget and the error is raised.
- Not configurable in v1: a cap is a safety bound, not a tuning knob.
  Revisit on demand (bd) with the timeout's default+validation pattern.

**Site 1 — genai stream loop** (`genai_adapter.rs` `chat_stream`): count
`chunk.content.len()` (UTF-8 bytes) per `ChatStreamEvent::Chunk`. If the
attempted cumulative count would exceed the cap, do NOT forward that chunk;
yield `Err(ResponseTooLarge { path: "chat_stream", .. })` and terminate — no
`Finished` event. Deltas delivered before the breach stay delivered (no
rollback). An oversized first delta forwards nothing. Dropping the stream
stops the SDK's forward progress; its internal capture is outside Camel's
bound (documented residual). The cap applies to every provider through this
adapter (openai, ollama, generic).

**Site 2 — materialized loop** (`producer.rs` ~:747): before
`full_text.push_str(&text)`, `checked_add` the accumulated byte count and the
delta's `len()`; breach → `Err(ResponseTooLarge { path:
"chat_materialized", .. })` propagating through the existing error arm —
the call fails, no partial `MaterializedResult`, tool-call state discarded
with the attempt. This covers the mock provider in materialized mode.
Direct streaming through the producer passthrough from non-genai providers
(the test mock, custom adapters) is NOT capped — recorded residual below.

**Site 3 — embed conversion boundary** (`genai_adapter.rs` `embed()`): after
`embed_batch` returns, sum `vector.len() * size_of::<f32>()` over decoded
vectors with `checked_add`; breach → `Err(ResponseTooLarge { path: "embed",
.. })`. The SDK has already allocated the JSON body and parsed vectors at
this point — the promise is that no `EmbedResponse` is returned and no
Camel-owned copy of the payload is made, not that the SDK never allocated.

**Residual (documented, not fixed here).** genai 0.6.5 reads embed JSON and
error bodies with `.text()` and buffers SSE internally — unreachable from
Camel without a fork or a new dependency (both out of bounds). Direct
streaming through the producer passthrough from non-genai providers (the
test mock, custom adapters) is not capped — the genai adapter site and the
materialized producer site are the enforced boundaries. Also: the caps
bound payload size only — a peer trickling bytes indefinitely still holds a
connection when no producer deadline is set (`timeout_secs: None` persists
indefinitely; the 600 s read-idle guard fires only on inactivity). ADR-0021
addendum records the new caps and this narrowed residual; a follow-up bd
issue is filed from `rc-kemrg` for the SDK-side bounds.

**Testing seams.** Chat over-cap tests use real ~8 MiB payloads (staged SSE
server / mock deltas — fast, no wall-clock). The embed over-cap test uses a
`#[cfg(test)]` cap override (precedent: `new_with_build_override` from 555)
so the real staged-server path runs with a tiny cap; production constants are
asserted by unit test. genai timeout tests untouched (no signature change to
`genai_adapter_tests.rs` helpers).

## Affected crates

- `camel-component-llm`: `error.rs` (new variant), `lib.rs` (cap constants),
  `producer.rs` (materialized check), `provider/genai_adapter.rs` (stream
  count + embed check + test override), plus tests; `docs/adr/0021` addendum;
  user-facing limits doc (if one documents the 555 bounds); spec delta
  `openspec/changes/llm-response-body-cap/specs/llm-provider-adapter/`.

## Architecture boundaries

Components-layer only: no Runtime/DSL/Services changes. `LlmError` is the
component's public taxonomy — the new variant follows the documented
error-taxonomy pattern; `From<LlmError> for CamelError` maps it through the
existing `ProcessorError` catch-all. Decision path, transport policy values,
SSRF/pinning and the lazy client build are untouched (hard limits).

## Phases

Single-phase. One coherent slice: error type + caps, three enforcement sites,
tests, docs/spec. Task order carries the dependency (error type first).
