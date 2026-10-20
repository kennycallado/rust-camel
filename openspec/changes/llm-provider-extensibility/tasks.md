# Tasks: llm-provider-extensibility

## Execution contract

This is one delivery slice with six ordered tasks and no phase headings. Scope check: accepted; all tasks serve the same provider-extension contract. Bd: rc-hh3cv.

Workers use `w_low` by default and `w_balanced` only for demonstrated cross-file difficulty. Reviews use `r_gpt`; the plan blessing uses `e_opus`, one attempt per dispatch. Do not merge, archive, close bd, bump versions, push, or add dependencies. Only task 1.2 requires a manifest change: add a feature using the existing optional genai dependency. Do not edit Cargo.lock or the CLI manifest.

Every cargo command below is a command body, not permission to run cargo directly. Run from `/home/shared/rust-camel-worktrees/372-llm-provider-extensibility`, never main, under `systemd-run --user --scope --collect --unit="mission372-$(date +%s%N)" -p MemoryMax=12G -p MemorySwapMax=2G -p TasksMax=1024 -p OOMPolicy=kill -- env TMPDIR=/home/shared/tmp CARGO_BUILD_JOBS=6`. Save the generated unit name before invocation, then append the command body. Check `pgrep -c compiled_artifa` before and after (both must report 0), record each command's exit status, and stop the saved unit with `systemctl --user stop` after completion. Tests using asynchronous waits must be bounded by a timeout, not sleeps.

Write each specified behavioral test before its production change. Record the actual failing assertion or missing-symbol compile failure, then implement and rerun. Regression/coverage-only tasks 1.3 and 1.4 may pass against the preceding implementation: record that honestly, do not manufacture a failure. If a needed case is absent from this plan, report `test-design-gap` rather than inventing a case. Each worker finishes with `cargo fmt --check --all` and `cargo clippy -p camel-component-llm --all-targets --all-features -- -D warnings` under containment. Review each result before advancing; legitimate findings are blocking.

## camel-component-llm configuration and runtime

### Task 1.1: Add the generic configuration contract without enabling networking

**Files:**
- `crates/components/camel-component-llm/src/config.rs` (modified)
- `crates/components/camel-component-llm/src/provider_factory.rs` (modified)
- `crates/components/camel-component-llm/src/lib.rs` (modified)

**Steps:**
1. Introduce crate-owned `LlmServiceKind` with `Anthropic` and `Gemini`, derived Clone/Copy/Debug/PartialEq/Eq/Deserialize and serde snake-case names. Introduce `GenericProviderConfig` with required `service`, `api_key`, `default_model`; optional `base_url`, `timeout_secs`, `max_concurrency`, `network_retry`, `pricing`, `cache_ttl_secs`, `cache_max_entries` using the existing OpenAI field types and serde defaults. Do not derive Serialize.
2. Introduce `GenericProviderConfig::validate_required_fields(&self) -> Result<(), &'static str>`, rejecting whitespace-only `api_key` or `default_model` with field-specific static diagnostics. Do not trim nonblank stored values. Give the nested struct a manual Debug implementation that redacts keys and credential-bearing URLs using the existing ADR-0051 URL-redaction helper.
3. Add `LlmProviderConfig::Generic(GenericProviderConfig)`, wire all six existing policy accessors and manual enum Debug, and re-export both new config types from `lib.rs`.
4. Call the required-field helper from `LlmGlobalConfig::validate`, converting failures to `CamelError::Config` with the provider name. Existing accessor-based checks reject zero limits and negative pricing for this variant without changing legacy validation.
5. Keep the factory exhaustive by adding a Generic arm: validate required fields, returning `LlmError::InvalidRequest` on failure; valid generic entries return `UnsupportedCapability` naming `generic-providers`. No generic networking feature exists at this task boundary; task 1.2 introduces it and replaces this arm with enabled/disabled dispatch. No placeholder constructor or SDK import is permitted.

**Tests:**
All new tests live in the existing config/factory unit-test modules. For config tests use `cargo test -p camel-component-llm --lib config::tests::generic_`; expected before implementation: missing types/variant or rejected `generic` tag; after: pass.
- `generic_services_parse_and_preserve_values`: setup two TOML provider entries (`type="generic"`, service anthropic/gemini, key ` test-key `, model `explicit-model`, no optional fields); action deserialize global config and validate; assert correct service variants, exact untrimmed key/model, optional fields None, and both map entries present.
- `generic_unknown_service_is_actionable`: setup service `anthropci`; action deserialize; assert error contains `anthropci`, `anthropic`, and `gemini`.
- `generic_required_fields_are_enforced`: setup each of service/api_key/default_model missing in turn; action deserialize; assert field-specific errors. Separately use keys/models `""` and `"  "` in programmatically constructed entries; action global validate; assert Config error names the missing-content field and not the nonblank secret from another field.
- `generic_debug_redacts_credentials_and_url`: setup API key `generic-secret-marker` and URL `https://user:password-marker@example.com/v1?api_key=query-secret-marker`; action Debug-format nested config, enum, and containing global config; assert none of the three secret markers or `password-marker` appears and redaction is visible. Do not change legacy Debug behavior in this task.
- `generic_policy_fields_round_trip_and_validate`: setup both services with timeout 7, concurrency 2, retry `{ enabled=true, max_attempts=3, initial_delay_ms=1, max_delay_ms=5, multiplier=2.0, jitter_factor=0.0 }`, pricing input/output 0.01/0.02, cache TTL 9, cache entries 4; action parse and call all accessors; assert exact values, including retry durations. Set each timeout/concurrency/cache limit to 0 separately and each price to -1 separately; assert validation rejects the named field.
- `generic_factory_rejects_blank_required_fields`: setup both service variants with a blank key or model and a harmless HTTPS override; action call private `build_single`; assert InvalidRequest names the field without exposing a nonblank key. Command: `cargo test -p camel-component-llm --lib provider_factory::tests::generic_factory_rejects_blank_required_fields`; expected missing generic support before, pass afterward.
- `generic_feature_disabled_is_actionable`: setup a valid generic entry; action deserialize and call `build_provider_map`; assert parsing succeeds and construction is UnsupportedCapability containing `generic-providers`. Command: `cargo test -p camel-component-llm --no-default-features --lib generic_feature_disabled_is_actionable`; expected before: parse error; after: pass. Task 1.2 restricts this test to builds without the new feature.

**Acceptance:**
- Each specified test passes; default and no-default feature builds compile.
- `cargo test -p camel-component-llm --lib` exits 0, including legacy configuration tests.
- No manifest or SDK-boundary change in this task; only concrete feature-disabled behavior is introduced.

- [x] 1.1

### Task 1.2: Enable native service targets behind one feature

**Files:**
- `crates/components/camel-component-llm/Cargo.toml` (modified)
- `crates/components/camel-component-llm/src/provider/mod.rs` (modified)
- `crates/components/camel-component-llm/src/provider/genai_adapter.rs` (modified)
- `crates/components/camel-component-llm/src/provider/genai_adapter_tests.rs` (modified)
- `crates/components/camel-component-llm/src/provider_factory.rs` (modified)

**Steps:**
1. Add `generic-providers = ["dep:genai"]` and include it in `all-providers`; retain default/mock/openai/ollama feature definitions and all dependency versions. Extend adapter module, Duration import, and adapter test-module cfg expressions to support generic-only builds.
2. Introduce `GenaiConfig::Generic { service: LlmServiceKind, api_key: String, base_url: Option<String> }` inside the adapter. Extend `adapter_kind`, `endpoint`, `api_key`, and `collect_pinning_from_config`: Anthropic uses AdapterKind::Anthropic and `https://api.anthropic.com/v1/`; Gemini uses AdapterKind::Gemini and `https://generativelanguage.googleapis.com/v1beta/`. Normalize overrides with the existing trailing-slash helper and pin overrides exactly as OpenAI does. Keep SDK imports confined to the existing allowlist.
3. Add `build_generic(name: &str, config: &crate::config::GenericProviderConfig, configured_timeout: Duration, policy: SsrfPolicy) -> Result<Arc<dyn LlmProvider>, LlmError>`, constructing the existing GenaiProvider with explicit config/model/timeout/policy. Use explicit AuthData::from_single through existing targeting; do not consult env vars or infer adapter kinds from models.
4. Replace task 1.1's Generic factory arm with cfg-enabled required-field validation first, then URL validation, effective timeout selection (`provider.or(global).unwrap_or(30)`), and `build_generic` dispatch. Retain the feature-disabled arm with its actionable error. Reject blank fields before URL/DNS validation or SDK initialization, including for programmatic callers. Gate `generic_feature_disabled_is_actionable` with absence of the new feature.
5. Make `GenaiProvider::supports_embed()` return false for Generic and true for existing configurations. At the start of `embed`, return UnsupportedCapability for Generic before initializing the OnceCell/client; leave existing embed flow and the public trait unchanged.

**Tests:**
Add tests to the existing adapter test file with a generic-only feature gate. Command: `cargo test -p camel-component-llm --no-default-features --features generic-providers --lib generic_`; expected before: unavailable feature/types/target or unsupported provider; after: pass.
- `generic_targets_are_explicit_and_normalized`: setup GenaiConfig Generic for each service with no URL, then override `https://example.com/native`; action inspect adapter kind, normalized endpoint, and resolved target for model `qwen3.5:4b`; assert correct native adapter, exact default URLs, override `https://example.com/native/`, unchanged model, and explicit nonempty auth. The model name must not select Ollama or OpenAI.
- `generic_factory_builds_chat_only_providers`: setup both service configs without URL overrides, timeout 7 and nonblank keys/models; action build provider map; assert exact provider ids, default models, supports_chat true and supports_embed false. No network request is needed to construct the lazy provider.
- `generic_embed_rejects_before_client_initialization`: setup each generic service with a BuildOverride that increments an atomic counter and returns an error if invoked; action call embed with one input; assert UnsupportedCapability and counter 0. This tests the direct trait path, not only endpoint validation.
- `generic_url_policy_and_pinning`: setup both Generic configs with overrides `https://127.0.0.1/v1`, `https://169.254.169.254/v1`, and `file:///tmp/llm`; action factory construction under PublicHttpsOnly; assert rejection for each. For `http://127.0.0.1:12345/native` under AllowInternal, action `collect_pinning_from_config`; assert None because IP literals need no DNS pinning; factory construction succeeds lazily. For `http://localhost:12345/native` under AllowInternal, assert Some with hostname `localhost`, nonempty loopback addresses, and resolver socket port 0, preserving the existing resolver semantics. Do not require DNS resolver results to carry the URL port; task 1.3 wiremock acceptance proves the actual configured server port is used.
- Existing mutation scenario `no_genai_imports_outside_adapter`: setup a snapshot of `src/provider/mod.rs`; temporarily add a real `use genai::Client;` import outside the allowlist; action `cargo test -p camel-component-llm --all-features --test boundary`; assert test failure names the disallowed file. Restore the exact snapshot even if the command fails, rerun, and assert exit 0. Never commit the injected import.

**Acceptance:**
- `cargo test -p camel-component-llm --no-default-features --features generic-providers --lib` exits 0.
- `cargo test -p camel-component-llm --no-default-features --lib generic_feature_disabled_is_actionable` exits 0.
- `cargo test -p camel-component-llm --all-features --test boundary` exits 0 after the mutation is restored.
- `cargo clippy -p camel-component-llm --no-default-features --features generic-providers --all-targets -- -D warnings` exits 0.
- The crate manifest diff contains only the required feature additions; no dependency/version/lockfile change.

- [x] 1.2

### Task 1.3: Pin native wire protocols, usage, and error mapping

**Files:**
- `crates/components/camel-component-llm/src/provider/genai_adapter_tests.rs` (modified)

**Steps:**
1. Introduce a `wiremock_generic` test module under the generic feature gate. Add bounded async fixtures directly in this module using existing wiremock, futures, and tokio dependencies. Build native providers through `build_provider_map` with `allow_internal=true`, not only direct adapter test constructors.
2. Add the native request/stream/error cases below, capturing text and the single Finished usage/model. Use one Anthropic `message_start` with usage input 3/output 0, content-block start, delta `hello`, block stop, message_delta stop_reason `end_turn` with output_tokens 2, then message_stop. Use one Gemini candidates frame with text `hello`, finishReason `STOP`, and usageMetadata promptTokenCount 3/candidatesTokenCount 2/totalTokenCount 5.

**Tests:**
For native cases: `cargo test -p camel-component-llm --no-default-features --features generic-providers --lib wiremock_generic`; expected baseline after task 1.2: pass, because this task locks protocol behavior rather than adding it. A failure is an implementation defect to resolve with review, not permission to weaken the assertions.
- `anthropic_factory_stream_captures_usage`: setup wiremock POST `/v1/messages`, Generic Anthropic base URL `{server.uri()}/v1`, key `anthropic-test-key`, default_model `claude-test`; action consume chat_stream with no model override; assert x-api-key equals key, anthropic-version exists, request model is `claude-test`, body stream=true, Delta text `hello`, exactly one Finished last, usage input/output/total 3/2/5, finished model `claude-test`, and stop finish reason.
- `anthropic_request_model_override_keeps_service`: setup the same Anthropic fixture with request model `qwen3.5:4b`; action consume chat_stream; assert POST remains `/v1/messages`, key header unchanged, JSON model and Finished model exactly `qwen3.5:4b`, and one request only.
- `gemini_factory_stream_captures_usage`: setup wiremock POST `/v1beta/models/gemini-test:streamGenerateContent` query alt=sse, Generic Gemini base URL `{server.uri()}/v1beta`, key `gemini-test-key`, model `gemini-test`; action consume chat_stream; assert x-goog-api-key equals key, expected path/query, Delta text `hello`, exactly one Finished last with 3/2/5 usage, model `gemini-test`, and stop finish reason.
- `gemini_resource_exhausted_is_retryable_rate_limit`: setup the Gemini endpoint answering HTTP 429 JSON `{ "error": { "code": 429, "status": "RESOURCE_EXHAUSTED", "message": "quota or throttling" } }`; action collect the error from chat_stream (allow the existing trait's initial or stream-item error surface); assert RateLimit retry_after None, is_retryable true, and no Finished.

**Acceptance:**
- All named tests pass without network outside wiremock or unbounded waits.
- `cargo test -p camel-component-llm --all-features` exits 0, preserving producer/cache/cost/mock suites without production changes to those modules.
- If a native fixture cannot be parsed, correct it against genai 0.6.5's native wire format without changing specified text, token counts, or error expectations.

- [x] 1.3

### Task 1.4: Pin policy propagation, legacy behavior, and quota retryability

**Files:**
- `crates/components/camel-component-llm/src/error.rs` (modified)
- `crates/components/camel-component-llm/src/provider_factory.rs` (modified, tests only)
- `crates/components/camel-component-llm/src/endpoint.rs` (modified, tests only)

**Steps:**
1. Add `quota_exhausted_not_retryable_while_rate_limit_is` to the existing error test module using the exact taxonomy below.
2. Add one rustdoc line to `is_retryable` stating QuotaExceeded is not retryable while RateLimit is retryable. Do not alter the match, variants, or non-exhaustive annotation.
3. Add factory legacy fixtures and an endpoint unit-test module if none exists. Use the private `resolve_provider` tuple to prove generic policy accessors reach the existing producer-composition boundary; do not add production getters or duplicate retry/semaphore wrappers.
4. Add endpoint embedding rejection for both generic services using the existing LlmComponent endpoint-construction path and a wiremock server to detect unintended I/O.

**Tests:**
- `quota_exhausted_not_retryable_while_rate_limit_is`: setup `LlmError::QuotaExceeded { detail: "billing limit".into() }` and RateLimit with retry_after None and Some(Duration::from_secs(7)); action call is_retryable on each; assert false/true/true respectively. Command: `cargo test -p camel-component-llm --lib quota_exhausted_not_retryable_while_rate_limit_is`; expected before production edit: pass, because existing behavior is correct; after: pass. Confirm the filtered command actually runs one test rather than zero.
- `generic_endpoint_embed_is_rejected`: setup each service through LlmComponent with a wiremock override and allow_internal=true; action create `llm:embed?provider=native`; assert creation fails with an unsupported-embedding diagnostic and the server has received zero requests. Command: `cargo test -p camel-component-llm --all-features --lib generic_endpoint_embed_is_rejected`; expected baseline and final: pass.
- `legacy_factory_variants_build_unchanged`: setup TOML OpenAI key/model with no URL override, Ollama base_url `http://127.0.0.1:11434`/model, and Mock response/default_model with global allow_internal=true; action parse and build under all-features; assert names/default models retained, chat true for all, embeddings true for OpenAI/Ollama, and Mock behavior consistent with existing tests. Command: `cargo test -p camel-component-llm --all-features --lib legacy_factory_variants_build_unchanged`; expected baseline and final: pass.
- `generic_endpoint_policy_tuple_preserves_settings`: setup both services with explicit timeout 7, concurrency 2, retry `{ enabled=true, max_attempts=3, initial_delay_ms=1, max_delay_ms=5, multiplier=2.0, jitter_factor=0.0 }`, pricing input/output 0.01/0.02, cache TTL 9, cache entries 4, and global timeout 99; action private resolve_provider on an endpoint with constructed provider map; assert tuple concurrency Some(2), provider timeout Some(7), retry exactly the supplied policy, pricing 0.01/0.02, cache Some((9, Some(4))). Repeat with provider timeout absent; assert tuple timeout None and global timeout remains Some(99), matching existing create_producer fallback. Command: `cargo test -p camel-component-llm --all-features --lib generic_endpoint_policy_tuple_preserves_settings`; expected baseline and final: pass.

**Acceptance:**
- The dedicated regression runs and passes; `cargo test -p camel-component-llm --lib error::tests` exits 0.
- All four named cases and `cargo test -p camel-component-llm --all-features` exit 0, including existing producer/cache/cost/mock suites.
- `LlmError` remains `#[non_exhaustive]`, with no behavioral or public-trait change.

- [x] 1.4

## Schema and operator documentation

### Task 2.1: Describe the generic entry with a scoped JSON Schema

**Files:**
- `schemas/llm-generic-provider.schema.json` (new)
- `schemas/README.md` (modified)
- `crates/components/camel-component-llm/tests/generic_provider_schema.rs` (new)

**Steps:**
1. Add a draft 2020-12 object schema for one provider entry with required type/service/api_key/default_model, type const generic, service enum anthropic/gemini, and nonblank strings for key/model. Include optional base_url string, positive integer timeout/concurrency/cache limits, and shared retry/pricing objects. Optional values may be absent; do not assert that TOML accepts JSON null.
2. Describe retry fields with their existing serialized names: enabled boolean; max_attempts and max_attempts_absolute nonnegative integers, with nullable max_attempts_absolute; initial_delay_ms/max_delay_ms nonnegative integer milliseconds; multiplier/jitter_factor numbers. Match existing serde field optionality, not new policy constraints. Pricing requires input_per_1k_tokens/output_per_1k_tokens, both numbers with minimum 0. Do not invent fields or replace generated route/metadata schemas. Preserve serde's existing unknown-field tolerance rather than adding additionalProperties=false.
3. Implement test-only runtime loading via `PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../schemas/llm-generic-provider.schema.json")`, parsed with existing serde_json. Do not embed the schema in library code or add a JSON Schema validator dependency. Parity tests inspect the schema and runtime parser explicitly; do not claim a full JSON Schema validation engine is running.
4. Add a schemas README section naming the schema's single-entry scope, hand-maintained source, relationship to config.rs, and the parity command. Existing generated-schema instructions remain unchanged.

**Tests:**
Command for all new cases: `cargo test -p camel-component-llm --test generic_provider_schema`; expected before: file missing/schema assertions fail; after: pass, without networking features.
- `generic_schema_discriminants_match_runtime`: setup runtime-loaded schema; action inspect root type, properties.type.const, service.enum, required fields, and parse JSON fixtures as LlmProviderConfig; assert object/generic, exact sorted service set `[anthropic, gemini]`, exact sorted required set `[api_key, default_model, service, type]`, both accepted fixtures deserialize into Generic with correct service, and `anthropci` fails with supported names.
- `generic_schema_required_strings_match_runtime`: setup a valid JSON fixture for each service; action omit each required property and attempt runtime deserialization; assert every omission fails. For blank key/model, deserialize then wrap in global config and validate; assert rejection. Assert schema key/model type string and pattern rejects whitespace-only content by checking the exact declared pattern `\\S` (JSON source escape for a non-whitespace regex) rather than adding a regex dependency.
- `generic_schema_policy_fields_match_runtime`: setup JSON fixture with timeout 7, concurrency 2, retry `{ enabled=true, max_attempts=3, initial_delay_ms=1, max_delay_ms=5, multiplier=2.0, jitter_factor=0.0 }`, pricing input/output 0.01/0.02, cache TTL 9, and cache entries 4; parse as LlmProviderConfig and inspect schema; assert all six policy field properties exist with the stated integer/object shapes, pricing minimum/required fields and retry serialized names agree, and runtime accessors produce the exact fixture values. Assert integer limit schema minima are 1 and pricing minima are 0; call global validate on zero limits and negative pricing fixtures and assert rejection.

**Acceptance:**
- All schema parity tests pass using existing dependencies and public config re-exports.
- `cargo xtask schema --check` exits 0 and generated canonical-route/component-metadata files remain unchanged.
- `git diff -- Cargo.lock` is empty; no new dependency or packaging-time library reference to the schema exists.

- [x] 2.1

### Task 2.2: Document generic services and complete mission verification

**Files:**
- `docs/src/components/llm.md` (modified)
- `crates/components/camel-component-llm/README.md` (modified)
- `crates/components/camel-component-llm/CONTEXT.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `docs/adr/0020-llm-component-provider-adapter-boundary.md` (modified)
- `crates/components/camel-component-llm/tests/generic_provider_schema.rs` (modified)

**Steps:**
1. Add parseable TOML examples for Anthropic and Gemini using type generic, service selector, explicit API key strings and default models, plus optional base URL and shared policy notes. Label any environment-variable substitution as application-level, not SDK discovery or direct serde expansion; do not add dependencies or new environment expansion code.
2. Document default native URLs, override normalization, x-api-key/x-goog-api-key authentication, explicit model/request override precedence, generic-providers/all-providers feature availability, compiled-out diagnostics, and chat-only embedding rejection. State that Anthropic's omitted max_tokens default is chosen by genai, not Camel.
3. Explain that Gemini RESOURCE_EXHAUSTED covers throttling and daily/project quota exhaustion and both map to RateLimit under the inherited mapping. Retry count follows the existing configured policy; max_attempts=0 can be unlimited unless an absolute cap is set. Contrast this with QuotaExceeded's fail-fast behavior without claiming every vendor quota response becomes QuotaExceeded.
4. Extend ADR-0020 with an English addendum preserving the SDK boundary, explaining the generic selector/gate decision, explicit ServiceTargets, and limitations. Update crate CONTEXT/README and CONTEXT-MAP's provider descriptions; define LLM service selector separately from Tower Service and the Services architecture tier. Do not rewrite historical ADR decisions or claim generic embeddings exist.
5. Run the mandatory STAGE 4 gate registry from AGENTS.md plus conductor additions, not a copied substitute. Record each exit status in bd/scoped review evidence, including the mission-mandated `feature_profiles` because task 1.2 changes a manifest. Keep source untouched when reporting pre-existing gate failures; exemptions require main reproduction and a linked bd issue under the conductor policy.

**Tests:**
- `generic_documentation_examples_parse`: add this named test to `crates/components/camel-component-llm/tests/generic_provider_schema.rs`. Setup read the LLM guide at test runtime using CARGO_MANIFEST_DIR-relative path; isolate a fenced TOML subsection for each generic service using explicit headings `### Anthropic generic provider` and `### Gemini generic provider`; action parse each first TOML code block as LlmGlobalConfig and validate; assert exactly one Generic entry with the heading's service, a nonblank example key/model, and no hidden environment expansion. Command: `cargo test -p camel-component-llm --test generic_provider_schema generic_documentation_examples_parse`; expected before: missing headings/examples; after: pass. Do not embed repository documentation in the library.
- `doc-guide-test`: setup updated docs and installed mdbook; action `mdbook test` with tool workdir `/home/shared/rust-camel-worktrees/372-llm-provider-extensibility/docs` under the same containment rule; assert exit 0. Expected before and after: existing executable guide examples remain valid; new examples are additionally checked by the parser test above.
- `doc-guide-build`: setup updated docs and installed mdbook; action `mdbook build` with tool workdir `/home/shared/rust-camel-worktrees/372-llm-provider-extensibility/docs` under the same containment rule; assert exit 0, as required by docs/AGENTS.md. Expected baseline and final: pass. Keep generated book output unstaged.
- `feature_profiles`: setup only the required LLM feature-map manifest change; action `cargo test -p camel-cli --test feature_profiles`; assert all feature profiles pass. Expected baseline and final: pass. Do not skip because no CLI manifest changed.
- `feature_matrix`: setup final crate; action run separately `cargo test -p camel-component-llm --no-default-features --features openai --lib`, `cargo test -p camel-component-llm --no-default-features --features ollama --lib`, `cargo test -p camel-component-llm --no-default-features --features generic-providers --lib`, and `cargo test -p camel-component-llm --no-default-features --lib`; assert each exit 0 and the disabled-feature test runs in the last command.

**Acceptance:**
- `cargo test -p camel-component-llm --all-features`, `cargo test -p camel-cli --test feature_profiles`, `cargo xtask schema --check`, `mdbook build`, and `mdbook test` exit 0.
- Every AGENTS.md quality gate plus conductor additions has an explicit exit status, valid N/A, or verified pre-existing exemption. Skip lint-commits only; do not run full `cargo test --workspace`.
- Final holistic `r_gpt` review and plan/spec hash checks remain conductor gates, not replaced by worker self-review. Park at the human gate; no merge/archive/close/push.

- [x] 2.2

## Coverage ledger

- SDK isolation and exact mutation scenario: 1.2; unchanged producer/cache/cost/mock suites: 1.3/1.4.
- Explicit native targets and legacy model non-inference: 1.2 plus existing OpenAI targeting regression; Anthropic model override: 1.3.
- Generic parsing, unknown service, missing/blank fields, secret redaction: 1.1; schema parity: 2.1.
- Feature-off construction error and generic-only build: 1.1/1.2; final legacy feature matrix: 2.2.
- Anthropic/Gemini stream usage and Gemini error mapping: 1.3.
- Unsupported embedding checks: 1.2 direct trait path and 1.4 endpoint path.
- URL validation/pinning: 1.2; policy accessor validation: 1.1; endpoint tuple propagation and legacy construction: 1.4.
- Dedicated quota versus rate-limit regression and documentation line: 1.4.
- Scoped schema and operator examples, service terminology, Gemini quota caveat, ADR addendum: 2.1/2.2.
