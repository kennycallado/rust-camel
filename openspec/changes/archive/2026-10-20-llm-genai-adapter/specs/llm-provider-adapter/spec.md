# llm-provider-adapter Specification (delta)

## ADDED Requirements

### Requirement: Provider adapter SDK isolation behind the ADR-0020 boundary

The LLM component SHALL confine all production `genai` SDK imports to the single
adapter file `provider/genai_adapter.rs`. `provider_factory.rs` SHALL select and
dispatch to adapter constructors WITHOUT importing `genai`; every other file in
the crate SHALL NOT import `genai`. The `LlmProvider` trait, all Camel-shaped
request/response types, `producer.rs`, `producer_cache.rs`, `cost.rs`,
`error.rs`, `mock.rs`, and `config.rs` SHALL NOT change.

#### Scenario: boundary test enforces the genai allowlist

- **GIVEN** `tests/boundary.rs` scanning all `.rs` files of the crate for the
  patterns `genai::` and `use genai` (word-boundary matched so `genai_adapter`
  path segments and identifiers do not themselves match) with its allowlist —
  `provider/genai_adapter.rs` and `provider/genai_adapter_tests.rs` ONLY
  (`provider_factory.rs` is NOT on the allowlist)
- **WHEN** a deliberate temporary `use genai::` import is added to a
  non-allowlisted file
- **THEN** the boundary test fails, and passes again after the violation is
  reverted

#### Scenario: untouched surfaces pass unchanged

- **GIVEN** the change swapped only the adapter, its tests, the boundary
  allowlist, and docs
- **WHEN** the full crate test suite runs (`cargo test -p
  camel-component-llm --all-features`)
- **THEN** every pre-existing test outside the adapter/boundary files passes
  without modification, including all `producer_*_tests.rs` and `tests/*`
  except `boundary.rs`

### Requirement: Chat stream maps genai events to Camel events with usage

The adapter SHALL set `capture_usage = true` on every chat stream request and
SHALL map `ChatStreamEvent::Chunk` text to `ChatEvent::Delta`, then any
complete tool calls (see the tool-call requirement), then terminate with one
`ChatEvent::Finished`. The stream event order SHALL be `Delta*` → `ToolCall*` →
`Finished`. `Finished` SHALL carry the observed `LlmUsage`, the mapped
`FinishReason`, and `model = Some(<resolved requested model>)` (recorded loss:
genai does not report the serving model). Reasoning chunks (`reasoning_content`)
SHALL be dropped (matching current behaviour). Usage derivation for chat SHALL
be: `None` when the prompt or completion token count is missing; otherwise
`total_tokens = total_tokens.unwrap_or(prompt + completion)`; negative counts
are treated as missing.

#### Scenario: OpenAI SSE deltas, usage and finish reason

- **GIVEN** a wiremock OpenAI-compatible server returning a canned SSE stream
  containing a `reasoning_content` chunk, text chunks, and a usage-carrying
  final chunk with `finish_reason`
- **WHEN** `chat_stream` consumes the stream
- **THEN** the reasoning chunk produces NO `Delta`, the text chunks produce
  `Delta` events in order, and the stream ends with exactly one `Finished`
  with usage, the mapped finish reason, and the requested model
- **AND** the captured request body contains
  `stream_options.include_usage=true`

#### Scenario: missing usage field yields no usage

- **GIVEN** a wiremock stream whose final chunk omits the completion token
  count
- **WHEN** `chat_stream` consumes the stream
- **THEN** `Finished.usage` is `None` (all-or-nothing per the derivation rule)

### Requirement: Tool calls are emitted exactly once per tool-call id, complete, at stream end

The adapter SHALL enable `capture_tool_calls` and SHALL emit one complete
`ChatEvent::ToolCall` per tool-call id (full arguments JSON) at stream end,
preserving today's per-id buffering so parallel tool calls in one turn each
produce their own event. It SHALL NOT emit on `ChatStreamEvent::ToolCallChunk`
(which carries partial argument deltas). Recorded genai limitation: the genai
OpenAI streamer reads only the FIRST tool call of each delta
(`delta_tool_calls.first()`); several tool calls packed into a single delta
would lose all but the first with no error signal — the adapter SHALL pin this
behaviour with a test so a future genai upgrade surfaces the change.

#### Scenario: chunked tool-call arguments collapse to one complete call

- **GIVEN** a wiremock OpenAI-compatible server streaming one tool call's
  arguments across multiple deltas
- **WHEN** `chat_stream` consumes the stream
- **THEN** exactly one `ToolCall` event is emitted for that id carrying the
  complete arguments JSON and the tool name

#### Scenario: two parallel tool calls produce two events

- **GIVEN** a wiremock OpenAI-compatible server streaming two distinct
  tool-call ids, each with chunked arguments
- **WHEN** `chat_stream` consumes the stream
- **THEN** two `ToolCall` events are emitted, one per id, each with complete
  arguments, before the single `Finished`

#### Scenario: multiple calls inside one delta pin the genai limitation

- **GIVEN** a wiremock server packing two tool calls into a single streaming
  delta
- **WHEN** `chat_stream` consumes the stream
- **THEN** only the first tool call is emitted (genai reads
  `delta_tool_calls.first()`) — the test documents and pins this known
  limitation

### Requirement: Embeddings return vectors in request order with usage

The adapter SHALL map genai embed responses so that output vector index order
matches the request input order, for both OpenAI and Ollama. The embed request
SHALL carry the per-request model, defaulting to the provider's
`default_model` (guarding Ollama's 501 on missing model). Embed usage SHALL be
derived as `completion_tokens = 0`, `total_tokens =
total_tokens.unwrap_or(prompt_tokens)`, with negative values treated as
missing. `EmbedResponse.metadata` SHALL be empty (recorded loss).

#### Scenario: OpenAI and Ollama embed vectors and usage

- **GIVEN** wiremock servers for `/v1/embeddings` (OpenAI) and `/api/embed`
  (Ollama) returning embeddings in a known order plus token counts
- **WHEN** `embed` is called with multiple inputs in a known index order
- **THEN** vectors are returned in the request index order with usage derived
  per the embed rule for both providers

#### Scenario: embed request body carries the model

- **GIVEN** an adapter configured with `default_model = "embeddinggemma"` and a
  request without an explicit model
- **WHEN** `embed` builds the genai request
- **THEN** the captured request body's `model` field is `embeddinggemma`

### Requirement: Provider errors map to the LlmError taxonomy with correct retryability

The adapter SHALL map genai errors onto the project `LlmError` taxonomy. On the
STREAM path it SHALL downcast the boxed `Error::WebStream { error }` to
`HttpError` (status/body) and to `reqwest::Error` (`is_timeout`/`is_connect`)
before mapping; an unmappable stream error falls to `LlmError::provider(..)`.
Mapping: 401 and 403 → `AuthFailed`; 404 for a missing model → `ModelNotFound`;
400 and 422 → `InvalidRequest`; 5xx → `ProviderUnavailable`; `StreamParse` →
`Protocol`; connect errors → `Network`; timeouts → `Timeout(configured)`. A
429 SHALL be disambiguated by body — quota detection matches `error.code ==
"insufficient_quota"` or `error.type == "insufficient_quota"` →
`QuotaExceeded` (NOT retryable); any other 429 → `RateLimit` (retryable). On
the stream path genai drops response headers, so chat-stream 429 maps to
`RateLimit { retry_after: None }` (same as today). On the embed path the
`Retry-After` header from `ResponseFailedStatus.headers` SHALL be honoured when
it is a delta-seconds value; an HTTP-date `Retry-After` maps to
`retry_after: None`. A mid-stream error SHALL surface as a mapped `Err` item
and SHALL NOT be followed by a `Finished` event.

#### Scenario: chat stream 429 is retryable without retry-after

- **GIVEN** a wiremock chat endpoint answering 429 with a rate-limit (non-quota)
  body
- **WHEN** `chat_stream` consumes the stream
- **THEN** the stream errors with `LlmError::RateLimit { retry_after: None }`
  and `is_retryable` is true

#### Scenario: embed 429 honours delta-seconds Retry-After

- **GIVEN** a wiremock embed endpoint answering 429 with header
  `Retry-After: 7` and a rate-limit body
- **WHEN** `embed` is called
- **THEN** the error is `LlmError::RateLimit { retry_after: Some(Duration::from_secs(7)) }`

#### Scenario: embed 429 with HTTP-date Retry-After maps to None

- **GIVEN** a wiremock embed endpoint answering 429 with an HTTP-date
  `Retry-After` header
- **WHEN** `embed` is called
- **THEN** the error is `LlmError::RateLimit { retry_after: None }`

#### Scenario: 429 insufficient_quota is not retryable

- **GIVEN** a wiremock endpoint answering 429 with a body whose `error.code` or
  `error.type` is `insufficient_quota`
- **WHEN** the error is mapped (either operation)
- **THEN** the result is `LlmError::QuotaExceeded` and `is_retryable` is false

#### Scenario: 401 auth and 503 unavailable

- **GIVEN** wiremock endpoints answering 401 and 503 respectively
- **WHEN** the errors are mapped
- **THEN** 401 yields `AuthFailed` and 503 yields `ProviderUnavailable`
  (retryable per `error.rs`)

#### Scenario: connection refused and timeout on chat stream

- **GIVEN** a chat endpoint that refuses connections, and a configured timeout
  exceeded against a stalled wiremock endpoint
- **WHEN** `chat_stream` runs against each
- **THEN** connection refused maps to `Network` (retryable) and timeout maps to
  `Timeout` carrying the configured duration (retryable)

### Requirement: SSRF hardening and lazy build-once are preserved

The adapter SHALL keep URL validation and DNS pinning
(`validate_llm_url`/`validate_llm_url_pinned`) and SHALL inject the hardened
reqwest client into genai (`with_reqwest`). The underlying client SHALL be
built lazily exactly once via the existing `OnceCell` pattern, with concurrent
calls sharing the cached client.

#### Scenario: SSRF tests ported green

- **GIVEN** the ported SSRF tests (blocked metadata address, scheme checks,
  AllowInternal policy rules)
- **WHEN** the adapter test suite runs
- **THEN** all ported SSRF tests pass with identical semantics

#### Scenario: client built once under concurrency

- **GIVEN** a `BuildOverride` seam returning a genai `Client` aimed at a
  wiremock endpoint and N concurrent first calls
- **WHEN** the calls race on the uninitialized `OnceCell`
- **THEN** the client build count is exactly 1 and all calls share the cached
  client

### Requirement: Explicit provider targeting without model-name guessing

The adapter SHALL resolve models via explicit
`ModelSpec::Target(ServiceTarget { endpoint, auth, model: ModelIden::new(
AdapterKind::OpenAI | AdapterKind::Ollama, model) })` derived from the
configured endpoint, and SHALL NOT rely on genai's model-name-based adapter
guessing.

#### Scenario: Ollama model name does not leak into OpenAI targeting

- **GIVEN** an OpenAI-feature adapter configured with model `qwen3.5:4b`
- **WHEN** a chat or embed request is built
- **THEN** the genai target is `AdapterKind::OpenAI` at the configured endpoint,
  never an inferred adapter kind

### Requirement: SDK swap recorded in docs and ADR-0020 amendment

The documentation surface SHALL stay consistent with the swap: crate
`CONTEXT.md` renames the `SiumaiProvider` term to the genai adapter,
`CONTEXT-MAP.md` updates the ADR-0020 summary line, the crate README provider
table reflects genai, ADR-0020 receives an amendment recording that the
boundary decision stands while the SDK changes to `genai =0.6.5` and that the
boundary allowlist is narrowed to the adapter + adapter tests (factory removed
as a no-import file), and `.github/workflows/integration-ollama-smoke.yml`
comment references update. (`docs/GLOSSARY.md` does not exist in the
repository; no glossary change is possible or required.)

#### Scenario: no stale siumai references in live docs

- **GIVEN** the docs surface above after the swap
- **WHEN** a case-insensitive search for `siumai` runs over those files
- **THEN** only intentional historical references remain (ADR-0020 body and its
  amendment's description of the superseded state)
