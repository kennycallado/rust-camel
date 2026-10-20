## MODIFIED Requirements

### Requirement: Provider adapter SDK isolation behind the ADR-0020 boundary

The LLM component SHALL confine production genai SDK imports to `provider/genai_adapter.rs`. The factory SHALL dispatch adapter constructors without importing the SDK. Configuration SHALL use crate-owned service selectors, not SDK types. `LlmProvider` and Camel request/response types SHALL remain unchanged. Configuration, factory, error documentation/tests, and feature gates MAY change to support additional providers while preserving existing OpenAI, Ollama, and Mock behavior. Producer, producer-cache, cost, and mock behavior SHALL remain unchanged.

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

- **GIVEN** the change added generic providers through configuration, factory dispatch, feature gates and docs
- **WHEN** the full crate test suite runs (`cargo test -p camel-component-llm --all-features`)
- **THEN** every pre-existing test outside the adapter/boundary files passes without modification, including all `producer_*_tests.rs` and `tests/*` except `boundary.rs`

#### Scenario: boundary scanner covers generic providers

- **GIVEN** generic providers implemented behind the adapter
- **WHEN** `tests/boundary.rs` scans the crate
- **THEN** only `provider/genai_adapter.rs` and `provider/genai_adapter_tests.rs` contain permitted SDK imports
- **AND** all existing provider and producer tests continue to pass

### Requirement: Explicit provider targeting without model-name guessing

The adapter SHALL construct explicit ServiceTargets from the configured service, endpoint, credential, and model. OpenAI, Ollama, Anthropic, and Gemini SHALL map to their corresponding SDK adapter kinds. The adapter SHALL NOT infer the service from model names. Request-level model selection SHALL override the configured default without changing the selected service.

#### Scenario: unfamiliar model remains on the configured service

- **GIVEN** a generic Anthropic provider and an explicitly overridden model name that resembles another provider's model
- **WHEN** the request is built
- **THEN** the request still targets the Anthropic protocol and configured endpoint with the exact requested model

#### Scenario: OpenAI model targeting remains unchanged

- **GIVEN** an OpenAI provider configured with model `qwen3.5:4b`
- **WHEN** chat or embed builds a request
- **THEN** the request targets OpenAI, never an inferred Ollama adapter

#### Scenario: Ollama model name does not leak into OpenAI targeting

- **GIVEN** an OpenAI-feature adapter configured with model `qwen3.5:4b`
- **WHEN** a chat or embed request is built
- **THEN** the genai target is `AdapterKind::OpenAI` at the configured endpoint,
  never an inferred adapter kind

## ADDED Requirements
### Requirement: Generic provider configuration is explicit and backward compatible

The component SHALL accept `type = "generic"` entries with `service = "anthropic"` or `service = "gemini"`, required `api_key` and `default_model`, optional `base_url`, and the existing provider timeout, concurrency, retry, pricing, and cache fields. Missing required strings SHALL fail deserialization; blank credentials or models SHALL fail validation/construction. Unknown service names SHALL fail deserialization with a diagnostic naming the rejected service and supported choices. Existing `openai`, `ollama`, and `mock` configurations SHALL retain their syntax, defaults, and behavior. Generic credentials SHALL be explicit, SHALL NOT trigger ambient SDK credential discovery, and SHALL NOT appear in Debug output or component-generated configuration diagnostics. Component-generated URL diagnostics SHALL use ADR-0051 redaction; provider response-body handling retains the existing error-mapping behavior.

#### Scenario: native protocols parse through one shape

- **GIVEN** generic Anthropic and Gemini entries with nonblank keys and default models
- **WHEN** global configuration is deserialized and validated
- **THEN** both entries retain their service, model, and shared settings

#### Scenario: misspelled service fails early

- **GIVEN** a generic entry with `service = "anthropci"`
- **WHEN** configuration is deserialized
- **THEN** parsing fails with `anthropci`, `anthropic`, and `gemini` in the diagnostic before any network call

#### Scenario: empty credential is not replaced from the environment

- **GIVEN** a generic configuration with a blank API key or blank default model
- **WHEN** validation or provider construction runs
- **THEN** configuration fails without SDK initialization, network access, or secret disclosure

#### Scenario: legacy configurations still build

- **GIVEN** unchanged OpenAI, Ollama, and Mock fixtures with their required features enabled
- **WHEN** the provider factory constructs them
- **THEN** construction succeeds with the same provider identifiers and capability behavior

### Requirement: Generic provider availability is feature controlled

The `generic-providers` feature SHALL enable the existing optional genai dependency, and `all-providers` SHALL include it. Default features and existing feature names SHALL remain unchanged. Generic configuration SHALL remain parseable without the feature, but provider construction SHALL fail with `UnsupportedCapability` naming `generic-providers`. Generic-only builds SHALL compile and serve both new protocols.

#### Scenario: feature disabled is a construction error

- **GIVEN** a valid generic provider entry in a build without `generic-providers`
- **WHEN** the entry is parsed and passed to the factory
- **THEN** parsing succeeds and construction returns an actionable feature-disabled error, not a compile error or fallback provider

### Requirement: Native chat protocols capture text and usage

Generic Anthropic and Gemini providers SHALL support chat streaming with base URL overrides, explicit service authentication, captured usage, and existing Camel event conversion. Both SHALL report embeddings unsupported and reject embed calls without I/O. Existing error mapping SHALL apply; Gemini 429 `RESOURCE_EXHAUSTED` SHALL map to retryable `RateLimit` with no stream retry-after value. The component SHALL NOT claim a new Gemini-specific quota taxonomy.

#### Scenario: Anthropic wiremock acceptance

- **GIVEN** a generic Anthropic provider built through the factory against a wiremock base URL with internal access explicitly allowed
- **WHEN** chat consumes an Anthropic SSE response
- **THEN** the request uses the configured model, `x-api-key`, and Anthropic messages path
- **AND** Camel emits the expected text and one Finished event with exact prompt/completion usage from the fixture

#### Scenario: Gemini wiremock acceptance

- **GIVEN** a generic Gemini provider built through the factory against a wiremock base URL with internal access explicitly allowed
- **WHEN** chat consumes a Gemini SSE response
- **THEN** the request uses the configured model, `x-goog-api-key`, and `streamGenerateContent` path with `alt=sse`
- **AND** Camel emits the expected text and one Finished event with exact prompt/completion usage from the fixture

#### Scenario: Gemini resource exhaustion is rate limiting

- **GIVEN** a Gemini wiremock endpoint answering HTTP 429 with `RESOURCE_EXHAUSTED`
- **WHEN** chat is attempted
- **THEN** it fails with retryable `RateLimit { retry_after: None }` and no successful Finished event

#### Scenario: generic embeddings fail clearly

- **GIVEN** either generic service
- **WHEN** capability checks or direct embed calls run
- **THEN** capability is false and embed returns `UnsupportedCapability` without a request

### Requirement: Generic providers preserve factory safety and policy composition

Generic base URL overrides SHALL undergo factory URL validation and adapter DNS pinning under the existing SSRF policy. Production defaults SHALL remain HTTPS at the documented native endpoints. Generic providers SHALL use existing timeout, retry, semaphore, pricing, and cache composition through provider-config accessors. No separate retry loop or concurrency implementation SHALL be introduced.

#### Scenario: private override is blocked by default

- **GIVEN** a generic provider whose override targets a loopback, private, or metadata address with the default public-HTTPS policy
- **WHEN** the factory constructs the provider
- **THEN** construction fails under the same rules as existing providers
- **AND** internal wiremock access requires the existing explicit internal-access policy

#### Scenario: shared policy fields are not dropped

- **GIVEN** generic entries with provider timeout, max concurrency, retry, pricing, and cache settings
- **WHEN** accessors and provider-map construction compose the provider
- **THEN** settings reach the existing wrappers with unchanged precedence and validation, including rejection of zero limits

### Requirement: Quota exhaustion has an explicit retryability regression

`QuotaExceeded` SHALL remain non-retryable, and `RateLimit` SHALL remain retryable regardless of whether retry-after is present. A dedicated named regression test and a retryability documentation line SHALL state this distinction. `LlmError` SHALL remain non-exhaustive.

#### Scenario: quota fails fast while throttling can retry

- **GIVEN** `QuotaExceeded`, `RateLimit { retry_after: None }`, and a RateLimit carrying a duration
- **WHEN** `is_retryable` is evaluated
- **THEN** quota returns false and both rate limits return true

### Requirement: Schema and operator documentation describe the generic path

A standalone JSON Schema at `schemas/llm-generic-provider.schema.json` SHALL describe one generic provider entry, including its discriminator, supported service names, required credentials/model, and shared policy fields. Schema tests SHALL check the service allowlist and required-field parity with runtime parsing using existing dependencies, reading the schema at test runtime rather than embedding it in library code. Its scope and maintenance SHALL be documented; generated route/endpoint schemas SHALL NOT be presented as global provider schemas. The LLM guide, crate README/CONTEXT, CONTEXT-MAP terminology, and ADR-0020 addendum SHALL document native endpoints, explicit credentials and model selection, feature availability, chat-only capability, and inherited retry policy. CONTEXT-MAP SHALL distinguish the LLM service selector from Tower Service and the Services architecture tier. The guide and ADR addendum SHALL explain that Gemini uses `RESOURCE_EXHAUSTED` for both throttling and daily/project quota exhaustion, so both receive bounded RateLimit retries under the existing mapping.

#### Scenario: schema agrees with configuration

- **GIVEN** the schema and runtime generic service selector
- **WHEN** parity tests inspect their supported service names and required fields and parse accepted/rejected fixtures
- **THEN** Anthropic/Gemini and required type/service/api_key/default_model agree and unknown services fail at runtime

#### Scenario: operator can configure either native protocol

- **GIVEN** the documented generic examples
- **WHEN** an operator follows the feature, credential, model, and base URL instructions
- **THEN** no legacy configuration migration or SDK knowledge is required
- **AND** examples distinguish application-level environment expansion from direct deserialization
