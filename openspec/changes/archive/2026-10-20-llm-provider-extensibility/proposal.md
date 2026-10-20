# Proposal: llm-provider-extensibility

## Why

The LLM component already pays the dependency cost for genai 0.6.5, but exposes only OpenAI, Ollama, and Mock. Users cannot select Anthropic or Gemini's native protocols through configuration. Bd: rc-hh3cv; mission 372.

## What Changes

- Add one `type = "generic"` configuration with a crate-owned service selector, initially `anthropic` and `gemini`. Do not add one provider-config variant per service.
- Route both services through the existing provider factory, hardened client, retry policy, semaphore, and `LlmProvider` boundary.
- Add the `generic-providers` feature to `all-providers`, without changing existing feature names or default features.
- Add a standalone JSON Schema for the new generic provider entry. Existing generated schemas describe routes and endpoint metadata, not global provider configuration; do not misrepresent them as provider-config schemas.
- Document configuration, explicit credentials, endpoints, models, chat-only support, and retryability. Add an ADR-0020 addendum.
- Add an explicit regression test for non-retryable quota exhaustion versus retryable rate limiting.

Affected crate: `camel-component-llm`. `camel-cli` feature-profile tests verify feature forwarding; no CLI behavior changes are intended.

Excluded: additional SDK dependencies, SDK upgrades, arbitrary SDK adapter names, new embedding protocols, environment-variable discovery, automatic tool execution, and public trait changes.

## Acceptance criteria

- Anthropic and Gemini chat streams work against wiremock with base URL overrides, explicit authentication, text, and captured usage; Gemini error mapping is tested.
- Unknown services fail during parsing; compiled-out services fail at provider construction with the required feature named.
- Existing OpenAI, Ollama, and Mock configurations remain unchanged and their suites pass.
- Generic overrides obey existing SSRF/DNS policy, timeout, retry, and concurrency composition.
- Generic schema and documentation match runtime configuration. The adapter boundary scanner passes.
- A named regression test proves `QuotaExceeded` is not retryable and `RateLimit` is retryable.

## Risk budget

Accept the SDK's existing stream limitations, including missing `Retry-After` headers. Do not weaken URL validation, expose credentials through Debug or serialization, add dependencies, or infer a protocol from model names. The generic path is chat-only; unsupported embedding requests must fail clearly. No merge, archive, issue closure, or push belongs to this mission.
