# Design: llm-provider-extensibility

## Approach

Add `LlmProviderConfig::Generic(GenericProviderConfig)`. Its `type` tag is `generic`; its required `service` is a crate-owned, serde snake-case `LlmServiceKind` enum with `Anthropic` and `Gemini`. Keep SDK types out of configuration. The allowlist rejects unknown services during deserialization, including when networking features are disabled. Future services extend this selector and adapter mapping, not the configuration-variant hierarchy. Define “LLM service selector” in CONTEXT-MAP separately from Tower Service and the Services architecture tier.

`GenericProviderConfig` requires `api_key` and `default_model`, and accepts optional `base_url`, `timeout_secs`, `max_concurrency`, `network_retry`, `pricing`, `cache_ttl_secs`, and `cache_max_entries` with existing types and defaults. Reject blank credentials and blank default models at validation/build time, including programmatically built configurations. Preserve credentials verbatim when nonblank. Redact credentials in Debug for both the nested struct and enum; do not add Serialize. Use existing URL diagnostic redaction for the new surface.

Credentials are explicit. The adapter must not discover environment variables. Documentation may show the existing application-level environment substitution for `ANTHROPIC_API_KEY` and `GEMINI_API_KEY`, but direct deserialization does not expand those strings. Models are explicit, have no provider-dependent default, and pass unchanged to the selected protocol; request model overrides retain precedence.

Inside `provider/genai_adapter.rs`, map the selector to explicit SDK ServiceTargets:

| Service | Adapter kind | Default endpoint | Authentication |
| --- | --- | --- | --- |
| anthropic | Anthropic | `https://api.anthropic.com/v1/` | explicit single key; SDK sends `x-api-key` |
| gemini | Gemini | `https://generativelanguage.googleapis.com/v1beta/` | explicit single key; SDK sends `x-goog-api-key` |

Honor base URL overrides through the existing endpoint normalization. Reuse stream conversion, usage capture, lazy OnceCell initialization, hardened reqwest injection, and error mapping. Both new services advertise `supports_embed() == false`; direct embed calls also return `UnsupportedCapability` without network I/O. Existing OpenAI/Ollama embedding support remains unchanged. Anthropic's SDK default for omitted `max_tokens` is documented, not reimplemented.

Add `generic-providers = ["dep:genai"]`; include it in `all-providers`. Default remains mock-only. Keep configuration types available unconditionally. With this feature absent, factory construction returns `UnsupportedCapability` naming `generic-providers`. Audit adapter-module/import/test cfg expressions so generic-only builds work. Existing `openai` and `ollama` gates retain their behavior.

Factory construction validates custom URLs before adapter dispatch. Generic targets use the same DNS pinning and policy-controlled client path as OpenAI, including internal-address rejection under the default policy. Generic accessors feed the existing timeout, retry, semaphore, pricing, and cache composition; do not create parallel wrappers.

Add `schemas/llm-generic-provider.schema.json`, a hand-maintained schema for one generic entry, not a replacement global-config schema. Cover the discriminator, service allowlist, required strings, and shared policy fields. Explain its scope and maintenance in `schemas/README.md`; test schema/service parity without adding dependencies. Tests read it at runtime using a CARGO_MANIFEST_DIR-relative path; library code must not embed it or require it for packaging. Generated route and component-metadata schemas remain unchanged unless their source actually changes.

This is one coherent delivery slice; no phase grouping is needed.

## Affected crates

- `camel-component-llm`: configuration, factory, adapter, tests, feature map, and crate docs.
- `camel-cli`: existing feature-profile test execution only.
- Shared docs: LLM guide, CONTEXT-MAP provider terminology, schema documentation, and ADR-0020 addendum. The guide and addendum must state that Gemini `RESOURCE_EXHAUSTED` covers both throttling and daily/project quota exhaustion; both map to retryable RateLimit within the configured attempt budget. Add one retryability line to ADR-0021 or `is_retryable` documentation.

## Architecture boundaries

ADR-0020 remains authoritative: production SDK imports stay in the adapter and the factory dispatches Camel-shaped constructors. Update the prior SDK-swap spec's historical freeze on config/error files so it does not prohibit this extension. ADR-0021 retry/semaphore composition and ADR-0051 credential redaction remain intact. Runtime, DSL, Services, Languages, and Functions gain no behavior or SDK dependency. Operator configuration stays on the control plane.

## Alternatives considered

- Bespoke Anthropic/Gemini provider-config variants duplicate shared policy and grow with every backend.
- SDK-shaped config or unrestricted service strings expose an implementation dependency and delay errors until requests.
- A gate named `genai` leaks the SDK into user-facing feature configuration.
- Automatic credential/model discovery makes deployments ambiguous and can select the wrong protocol. Explicit keys and targets avoid that risk.
