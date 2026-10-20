# Design: llm-genai-adapter

## Approach

Swap the SDK behind the ADR-0020 boundary. The project-owned `LlmProvider` trait,
all Camel-shaped types, the producer, cache, cost observability, retry and
concurrency control stay exactly as they are; only the single adapter file plus
its tests change SDK.

`genai_adapter.rs` (renamed from `siumai_adapter.rs`) keeps the existing shape:
config enum (`OpenAi`/`Ollama`), lazy `tokio::sync::OnceCell<CachedClient>`,
`build_openai`/`build_ollama` signatures in `provider_factory.rs`, SSRF URL
validation + DNS pinning (`validate_llm_url_pinned`). The genai client is built
with `with_reqwest(...)` so the hardened, DNS-pinned reqwest client is injected
unchanged (genai `builder.rs` supports this; reqwest 0.13 matches the workspace).

API-fit rules (from the binding e_opus investigation, corrected against the
genai 0.6.5 source during spec-bless):

1. `ChatOptions.capture_usage = true` on every chat call — without it OpenAI
   never sends `include_usage` and cost observability plus the cache's stored
   usage break silently.
2. Tool calls arrive at STREAM END: enable `capture_tool_calls` and emit one
   complete `ChatEvent::ToolCall` PER tool-call id from the finished message
   (per-id buffering, as today — parallel calls each get an event). Do NOT emit
   on `ChatStreamEvent::ToolCallChunk` (partial deltas). Known genai
   limitation to pin with a test: the OpenAI streamer reads only
   `delta_tool_calls.first()`, so multiple calls packed into one delta lose
   all but the first without a signal.
3. Stream-path error extraction: for `Error::WebStream { error }` downcast the
   boxed error to `genai::Error` (match `HttpError { status, body, .. }`,
   status via `as_u16()`) then to `reqwest::Error`
   (`is_timeout`/`is_connect`); on the non-stream path decompose
   `Error::WebModelCall`/`Error::WebAdapterCall` →
   `webc::Error::ResponseFailedStatus { status, body, headers }` (status/body
   mapping + `Retry-After` from headers) and `webc::Error::Reqwest`; map
   top-level `HttpError` too. Without this, 401/429/5xx all collapse into
   the non-retryable `Provider` catch-all. 429 disambiguation by body —
   `error.code`/`error.type == "insufficient_quota"` → `QuotaExceeded` (not
   retryable); otherwise `RateLimit`. genai drops stream response headers, so
   chat-stream 429 is `RateLimit { retry_after: None }` (same as today);
   `Retry-After` (delta-seconds; HTTP-date → `None`) is honoured only on the
   embed path via `ResponseFailedStatus.headers`. This is the one intended
   retry-behaviour change (today `retry_after` is always `None`).
4. Model resolution: always
   `ModelSpec::Target(ServiceTarget { endpoint, auth, model: ModelIden::new(
   AdapterKind::OpenAI | AdapterKind::Ollama, model) })` — never let genai
   guess the provider from the model name (e.g. `qwen3.5:4b`).
5. `Delta` events come from `ChatStreamEvent::Chunk`; reasoning
   (`reasoning_content`) chunks are dropped (same as today). Event order:
   `Delta*` → `ToolCall*` → `Finished`. `Finished.model` = the resolved
   requested model (genai does not report the serving model).
6. Usage derivation: chat — `None` unless both prompt and completion counts
   are present, `total = total_tokens.unwrap_or(p + c)`; embed —
   `completion = 0`, `total = total_tokens.unwrap_or(prompt)`;
   negative `i32` counts are treated as missing. `EmbedResponse.metadata`
   is empty. Embed requests carry the per-request model defaulting to
   `default_model` (guards Ollama 501 on missing model).

Error mapping (bottom of file, table-tested): genai `Error` → our `LlmError` —
401/403→`AuthFailed`, 404 model→`ModelNotFound`, 400/422→`InvalidRequest`,
5xx→`ProviderUnavailable`, `StreamParse`→`Protocol`, connect→`Network`,
timeouts→`Timeout(configured)`, everything else→`LlmError::provider(..)`. A
mid-stream error surfaces as a mapped `Err` item with no trailing `Finished`.

Test seam: the current `BuildOverride` returns siumai trait objects that genai
cannot supply. The override becomes a closure returning a `genai::Client`
constructed against a `wiremock` base URL (already a workspace dev-dep), which
keeps the `client_built_once_under_concurrency` test meaningful with real genai
types.

Boundary tightening: the `tests/boundary.rs` allowlist narrows to
`provider/genai_adapter.rs` + `provider/genai_adapter_tests.rs`
(`provider_factory.rs` leaves the allowlist — it imports no SDK). Scan patterns
become `genai::` and `use genai` with a word boundary so `genai_adapter`
identifiers and path segments do not match. The ADR-0020 amendment records the
narrowing.

## Affected crates

- `camel-component-llm`: Cargo.toml (dep swap), `provider/genai_adapter.rs`
  (rewrite), `provider/mod.rs` + `provider_factory.rs` + `lib.rs` (names/docs),
  `provider/genai_adapter_tests.rs` (rewrite + new wiremock tests),
  `tests/boundary.rs` (allowlist), README/CONTEXT.md.
- Repo-level docs: `CONTEXT-MAP.md`, `docs/adr/0020-*.md` (amendment),
  `.github/workflows/integration-ollama-smoke.yml` (comment only).
- `camel-cli`: no change (it forwards `all-providers`, which keeps existing
  semantics).

## Architecture boundaries

Components layer only (`camel-component-llm`). The data/control plane split,
the endpoint/producer pipeline, and the `LlmProvider` trait are untouched.
ADR-0020's decision stands — one production file holds all SDK imports; the
amendment records the SDK change siumai → genai and keeps `tests/boundary.rs`
as the enforcement mechanism, now scanning for `genai::`.

## Alternatives considered

- Keep siumai, bump beta.11: dead branch — beta.10 removed the imported traits
  and the Ollama provider, MSRV 1.95. Rejected (e_opus verdict).
- async-openai alone: viable fallback (MSRV 1.75, 126 nodes) but forces Ollama
  through the OpenAI-compat layer with strict OpenAI-shaped types and a
  streaming-usage gap. Kept on file as fallback, not chosen.
- rig-core: framework whose agent abstractions overlap our EIPs; MSRV 1.95.
  Rejected.
- Raw reqwest: we would own SSE + NDJSON + tool-call accumulation for two
  protocols; kept as the exit path behind ADR-0020, not chosen now.
