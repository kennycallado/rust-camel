# Proposal: llm-genai-adapter

## Why

The `siumai` dependency (pinned `=0.11.0-beta.9`) is a dead branch: beta.10 removed
the capability traits our adapter imports (`ChatCapability`, `EmbeddingCapability`,
`EmbeddingExtensions`) and the `FinishReason` type, removed the Ollama provider
entirely (`siumai-provider-ollama` stops at beta.9), and raised MSRV to 1.95 while
this workspace declares 1.89. Bumping is a full rewrite with native-Ollama loss.
Staying frozen on beta.9 carries perpetual-beta churn risk with no upgrade path.

e_opus investigation (bd rc-52re8, session `ses_eab90ad03ffeM0QdXyKP1qHOGP`,
binding) verified that `genai =0.6.5` (STABLE, not the 0.7.0-rc) covers both
operations we need — `chat_stream()` and `embed()` — with 146 dep nodes vs siumai's
166, and accepts our hardened reqwest client via `with_reqwest`, preserving the
SSRF posture of ADR-0020. The decision: EXIT siumai.

## What Changes

- `camel-component-llm/Cargo.toml`: drop the 4 siumai deps; add
  `genai = { version = "=0.6.5", optional = true }`; features `openai`/`ollama`
  become `["dep:genai"]`, `all-providers = ["openai","ollama"]`.
- `src/provider/siumai_adapter.rs` → `genai_adapter.rs`: rewrite conversions and
  error mapping. KEEP the SSRF validation/DNS pinning, lazy `OnceCell`, and
  `build_openai`/`build_ollama` signatures.
- `src/provider/siumai_adapter_tests.rs` → rewrite against genai (pure conversion
  tests ported; lazy-build test re-seamed; wiremock-based acceptance tests added).
- `src/provider/mod.rs`, `src/provider_factory.rs`, `src/lib.rs`: module/doc
  references only. **The `LlmProvider` trait does not change.**
- `tests/boundary.rs`: allowlist and scanned patterns switch `siumai` → `genai::`.
- Docs: crate `CONTEXT.md` (`SiumaiProvider` term), `CONTEXT-MAP.md`, crate README,
  ADR-0020 amendment, `.github/workflows/integration-ollama-smoke.yml` comment.
  (`docs/GLOSSARY.md` does not exist in the repo — cited surface is void.)

Explicitly UNCHANGED (must be proven): `producer.rs`, `producer_cache.rs`,
`cost.rs`, `error.rs`, `mock.rs`, `config.rs`, all `producer_*_tests.rs`, all
`tests/*` except `boundary.rs`. No component API change. No product behaviour
change except the declared embed-path `Retry-After` exception (Risk budget).

## Acceptance criteria

- Wiremock tests: OpenAI SSE → `Delta`×N + `Finished` with usage and finish
  reason; request body contains `stream_options.include_usage=true`.
- Chunked tool-call arguments produce exactly ONE complete `ToolCall`; two
  parallel tool-call ids produce two events.
- Ollama native NDJSON `prompt_eval_count`/`eval_count` → usage (chat stream
  AND embed; wiremock-covered even where the spec scenarios only name OpenAI).
- Embeddings: vector ORDER matches request index order, usage populated (both
  providers).
- 429 `insufficient_quota` → `QuotaExceeded` (not retryable); 429 rate-limit →
  `RateLimit` (retryable; honours `Retry-After` on the embed path only —
  chat-stream `retry_after: None`); 401 → `AuthFailed`; 503 →
  `ProviderUnavailable`; mid-stream error → mapped `Err` with no trailing
  `Finished`.
- A mid-stream error surfaces as a mapped `Err` item with NO trailing
  `Finished` event (wiremock test).
- SSRF tests and build-once-under-concurrency test ported and green.
- `tests/boundary.rs` fails on a deliberate `genai::` violation outside the
  allowlist (verified, then reverted).
- Live Ollama suite green including `ollama_cost_computed`.
  WAIVER (post-implementation): `ollama_cache_hit` + `ollama_cost_computed`
  fail for a PRE-EXISTING reason reproducible on main (both build
  `LlmGlobalConfig` without `allow_internal: true`; the factory's eager
  `validate_llm_url` on the Ollama arm is byte-identical to main) — filed as
  bd `rc-ls3g6`. The other 6 live tests, including `ollama_embed` and
  `ollama_chat_streaming`, are GREEN against real Ollama.

## Risk budget

Acceptable: recorded small losses (`Finished.model` becomes the requested model;
`EmbedResponse.metadata` empty; reasoning chunks dropped — same as today).
Intended retry-behaviour exception: today `RateLimit.retry_after` is always
`None`; this change honours `Retry-After` on the EMBED path (delta-seconds
values only) — the chat-stream path stays `retry_after: None` because genai
drops stream response headers.
Unacceptable: silent usage loss (`capture_usage = true` is mandatory), tool-call
double emission or dropped parallel tool calls, any other change to
retry/cache/cost behaviour, trait or component API changes.
