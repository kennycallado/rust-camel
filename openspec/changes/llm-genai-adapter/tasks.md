# Tasks: llm-genai-adapter

Single-phase change (design.md has no `## Phases` section). All tasks are in
dependency order; the tree compiles and the crate test suite is green at the
end of every task. genai API references are against the pinned `genai-0.6.5`
sources (`~/.cargo/registry/src/*/genai-0.6.5/`): `AuthData::Key` /
`AuthData::from_single(..)` (absent key → `AuthData::None`), `StopReason`
(variants `Completed`, `MaxTokens`, `ToolCall`, `ContentFilter`,
`StopSequence`, `Other` — each carries the raw string), `genai::chat::Usage`,
`genai::chat::Tool` (`Tool.name` is a `ToolName` enum), builder methods
`with_capture_usage` / `with_capture_tool_calls`. If any path or signature
differs, follow the real 0.6.5 source and record the adjustment in the task
report.

## camel-component-llm

### Task 1.1: Add genai dep + pure conversion/error-mapping layer (siumai still active)

**Files:**
- `crates/components/camel-component-llm/Cargo.toml` (modified)
- `Cargo.lock` (modified — genai enters the lockfile)
- `crates/components/camel-component-llm/src/provider/genai_adapter.rs` (new)
- `crates/components/camel-component-llm/src/provider/genai_adapter_tests.rs` (new)
- `crates/components/camel-component-llm/src/provider/mod.rs` (modified — test-gated module decl for the new module, see step 4)

**Steps:**
1. In `Cargo.toml` add `genai = { version = "=0.6.5", optional = true }` to
   `[dependencies]`, and append `"dep:genai"` to the EXISTING `openai` and
   `ollama` feature arrays so the dep enters the build graph
   (`openai = ["siumai/openai", "siumai-core", "siumai-provider-openai", "dep:genai"]`,
   same pattern for `ollama`; also append `"dep:genai"` to `all-providers`
   so every feature that compiles the new module activates the dep). Do NOT
   REMOVE any siumai entries yet — that is the atomic 1.2b flip.
2. Create `provider/genai_adapter.rs` with a doc header stating it will become
   the ONLY file importing genai (ADR-0020 boundary), and these NEW
   crate-visible pure items:
   - `pub(super) fn map_chat_message(m: &ChatMessage) -> genai::chat::ChatMessage`
     — role mapping (System/User/Assistant/Tool); assistant tool_calls
     forwarded as genai tool calls; tool-role messages map to genai tool
     responses with `tool_call_id` preserved.
   - `pub(super) fn map_tool_definition(d: ToolDefinition) -> genai::chat::Tool`
     — name (via `ToolName::from`), description, parameters JSON object.
   - `pub(super) fn map_tool_choice(c: &ToolChoice) -> genai::chat::ToolChoice`
     — Auto/None/Specific(name).
   - `pub(super) fn build_chat_request(req: &ChatRequest) -> genai::chat::ChatRequest`
     — messages, system prompt, tools (+ `ToolChoice`), max_tokens, temperature.
   - `pub(super) fn convert_usage(u: &genai::chat::Usage) -> Option<LlmUsage>`
     — chat rule: `None` unless prompt AND completion are present (negative
     i32 counts count as missing); `total = total_tokens.unwrap_or(p + c)`.
   - `pub(super) fn convert_embed_usage(prompt: Option<i32>, total: Option<i32>) -> Option<LlmUsage>`
     — embed rule: `completion = 0`, `total = total.unwrap_or(prompt)`, `None`
     if prompt missing, negatives missing.
   - `pub(super) fn map_finish_reason(r: &genai::chat::StopReason) -> FinishReason`
     — `Completed`/`StopSequence` → `Stop` (note the StopSequence fold in the
     doc comment), `MaxTokens` → `Length`, `ToolCall` → `ToolCall`,
     `ContentFilter` → `ContentFilter`, `Other(s)` → `FinishReason::Other(s)`.
   - `pub(super) fn resolve_target(kind: AdapterKind, endpoint: &str, api_key: Option<&str>, model: &str) -> genai::ModelSpec`
     — normalises the endpoint to always end in `/` (a missing trailing slash
     corrupts genai URL joins: `…11434api/chat`), `ModelSpec::Target(
     ServiceTarget { endpoint, auth, model: ModelIden::new(kind, model) })`;
     auth = `AuthData::from_single(api_key)` when present, `AuthData::None`
     otherwise. NEVER a plain model string. Callers pass the OpenAI default
     `https://api.openai.com/v1/` when `base_url` is `None` (see
     `openai_endpoint_for` below).
   - `pub(super) fn openai_endpoint_for(base_url: Option<&str>) -> String`
     — `Some(u)` → normalised-with-trailing-slash `u`; `None` →
     `https://api.openai.com/v1/`.
   - `pub(super) fn embed_model_for(req: &EmbedRequest, default: &str) -> String`
     — per-request model or the provider default (guards Ollama 501).
   - `pub(super) fn map_genai_error(err: &genai::Error, configured_timeout: Duration) -> LlmError`
     with THREE transport arms:
     a. `split_web_stream_error(err)` — for `Error::WebStream { error, .. }`
        (the variant also carries `model_iden`/`cause`):
        `error.downcast_ref::<genai::Error>()` and match
        `Error::HttpError { status, body, .. }` (status via `as_u16()`); then
        `error.downcast_ref::<reqwest::Error>()` (`is_timeout()`,
        `is_connect()`).
     b. `split_webc_error(err)` — decompose the two webc-carrying variants
        `Error::WebModelCall { model_iden, webc_error }` and
        `Error::WebAdapterCall { adapter_kind, webc_error }`:
        `webc::Error::ResponseFailedStatus { status, body, headers }` (status
        mapping + `is_insufficient_quota` + `parse_retry_after(headers)`;
        for 404 → `ModelNotFound` take the model from
        `model_iden.model_name`) and `webc::Error::Reqwest(reqwest::Error)`
        (timeout/connect).
     c. direct/top-level variants: `Error::HttpError { status, .. }` →
        status mapping via `status.as_u16()`; `StreamParse` → `Protocol`;
        `Unsupported*` → `UnsupportedCapability`.
     Shared status mapping once extracted: 401/403 → `AuthFailed { detail }`;
     404 (model) → `ModelNotFound(model)`; 400/422 → `InvalidRequest`; 5xx →
     `ProviderUnavailable`; other 4xx → `LlmError::provider(..)`. reqwest
     timeout → `Timeout(configured_timeout)`; connect → `Network(msg)`.
   - `pub(super) fn is_insufficient_quota(status: u16, body: &str) -> bool`
     — true only when status == 429 AND the body JSON has
     `error.code == "insufficient_quota"` or `error.type == "insufficient_quota"`
     (non-JSON bodies → false).
   - `pub(super) fn parse_retry_after(value: Option<&str>) -> Option<Duration>`
     — delta-seconds digits only; `None` for HTTP-date forms, garbage, absent.
3. Create `provider/genai_adapter_tests.rs` and include it as a CHILD module
   of `genai_adapter.rs` via `#[cfg(all(test, feature = "openai"))]
   #[path = "genai_adapter_tests.rs"] mod tests;` (sibling modules cannot see
   the private `GenaiProvider`; this mirrors siumai_adapter.rs's existing
   `#[path]` pattern) — the tests listed below go in that file.
4. In `provider/mod.rs` declare the module TEST-ONLY until the 1.2b flip:
   `#[cfg(all(test, any(feature = "openai", feature = "ollama", feature = "all-providers")))] mod genai_adapter;`
   (a plain `cfg(test)` would break the default mock-feature build; a
   non-test decl would drag dead code into production builds at 1.1/1.2a).
   Do NOT touch `siumai_adapter` wiring in this task.

**Tests:** (command: `cargo test -p camel-component-llm --features openai,ollama genai_adapter`; expected: RED first — functions/tests missing — then GREEN)
- `map_tool_definition_function_name_and_description`: `ToolDefinition` name `get_weather` + description + parameters object → genai Tool carries identical name/description/parameters.
- `map_tool_choice_auto_none_specific`: three `ToolChoice` values → genai Auto / None / Function(name == "get_weather").
- `convert_chat_message_tool_role_keeps_tool_call_id`: tool-role ChatMessage + tool_call_id → genai tool response with same id and content.
- `convert_chat_message_assistant_forwards_tool_calls`: assistant message with one tool call → genai assistant message carries the tool call (name + arguments string).
- `chat_usage_missing_completion_is_none`: Usage prompt 10, completion missing → `None`.
- `chat_usage_totals_default_to_sum`: prompt 10, completion 5, total missing → `Some(LlmUsage { 10, 5, 15 })`.
- `chat_usage_negative_counts_are_missing`: prompt -1 → `None`.
- `embed_usage_completion_zero_total_defaults_to_prompt`: prompt 7, total missing → `Some(LlmUsage { 7, 0, 7 })`.
- `finish_reason_maps_all_variants`: each StopReason variant → Stop (incl. StopSequence)/Length/ToolCall/ContentFilter; Other string → `Other`.
- `resolve_target_openai_kind_for_ollama_named_model`: `resolve_target(AdapterKind::OpenAI, "https://api.example.com/v1", Some("sk"), "qwen3.5:4b")` → `ModelSpec::Target` with `AdapterKind::OpenAI`, endpoint and model name preserved (NO name-based inference).
- `resolve_target_ollama_auth_none`: kind Ollama, no api key → `AuthData::None` equivalent.
- `resolve_target_appends_trailing_slash`: endpoint `http://localhost:11434` → target endpoint string ends with `/` (no corrupted join like `11434api/chat`).
- `openai_default_endpoint_when_base_url_none`: `openai_endpoint_for(None)` → `https://api.openai.com/v1/`; `Some("http://host:9/v1")` → `http://host:9/v1/`.
- `embed_model_defaults_to_provider_default`: empty request model + default `embeddinggemma` → `"embeddinggemma"`; explicit request model wins.
- `map_error_401_403_authfailed`: HttpError status 401 then 403 → `LlmError::AuthFailed`.
- `map_error_404_model_not_found`: status 404 + model in message → `ModelNotFound`.
- `map_error_400_422_invalid_request`: statuses 400, 422 → `InvalidRequest`.
- `map_error_5xx_provider_unavailable`: statuses 500, 503 → `ProviderUnavailable`.
- `map_error_timeout_and_connect`: construct real reqwest errors WITHOUT wiremock (not a dev-dep yet): connect error = request to a port that was bound then dropped (listener closed); timeout = `TcpListener` that accepts and stays silent with the test client at `.timeout(50ms)` — all waits bounded, every test reqwest client uses `.no_proxy()` → `Timeout(configured)` / `Network`.
- `map_error_stream_parse_protocol`: `Error::StreamParse` → `Protocol`.
- `webc_response_failed_status_maps_status_and_retry_after`: `Error::WebModelCall` wrapping `ResponseFailedStatus { status: 429, body: rate-limit JSON, headers: [("Retry-After", "9")] }` → `RateLimit { retry_after: Some(9s) }`; same with HTTP-date header → `retry_after: None`.
- `webc_reqwest_timeout_maps_to_timeout`: `Error::WebModelCall` wrapping `webc::Error::Reqwest` timeout → `Timeout(configured)`.
- `web_adapter_call_variant_maps_identically`: `Error::WebAdapterCall` wrapping the same `ResponseFailedStatus` 503 payload → `ProviderUnavailable` (both webc variants share the mapping).
- `insufficient_quota_detected_by_code_and_type`: 429 + `{"error":{"code":"insufficient_quota"}}` → true; 429 + `{"error":{"type":"insufficient_quota"}}` → true; 429 + `{"error":"rate limit"}` → false; non-JSON → false; status 500 + quota body → false.
- `retry_after_delta_seconds_parsed_http_date_rejected`: `Some("7")` → `Some(Duration::from_secs(7))`; `Some("Wed, 21 Oct 2026 07:28:00 GMT")` → `None`; `Some("soon")` → `None`; `None` → `None`.

**Acceptance:**
- `cargo check -p camel-component-llm --features openai,ollama` exits 0 (siumai path untouched).
- New unit tests pass; `cargo test -p camel-component-llm` (mock default) still passes — no existing file changed except `mod.rs` cfg(test) decls.
- No `unwrap()` in new code (`cargo xtask lint-unwrap` passes).

- [x] 1.1

### Task 1.2a: GenaiProvider struct + LlmProvider impl (unwired, alongside siumai)

**Files:**
- `crates/components/camel-component-llm/src/provider/genai_adapter.rs` (modified — provider struct + `LlmProvider` impl)

**Steps:**
1. Add private `GenaiConfig` enum (`OpenAi { api_key, base_url, model }` /
   `Ollama { base_url, model }`) and private `GenaiProvider` struct: fields
   `id`, `default_model`, `config`, `policy: SsrfPolicy`,
   `configured_timeout: Duration`, `client: Arc<tokio::sync::OnceCell<genai::Client>>`,
   test-only `client_builds: Arc<AtomicUsize>` + `build_override:
   Option<BuildOverride>` with `type BuildOverride = Arc<dyn Fn() ->
   BoxFuture<'static, Result<genai::Client, LlmError>> + Send + Sync>`
   (test-only). Constructors `new(id, default_model, config,
   configured_timeout, policy)` and `#[cfg(test)]
   new_with_build_override(id, default_model, config, configured_timeout,
   override_, policy)`.
2. `build_client_from_config`: port the EXISTING hardening unchanged —
   copy the private `collect_pinning_from_config` (siumai_adapter.rs) INTO
   `genai_adapter.rs` (sibling modules cannot call it; the duplicate is
   temporary — 1.2b deletes the siumai file) still going through
   `crate::provider_factory::validate_llm_url_pinned`, then
   `crate::hardened_http_client_with_pinning(..)` / `hardened_http_client()`
   exactly as today (redirect policy none, `.no_proxy()`, connect timeout,
   fixed timeout — do NOT add a new `.timeout(configured_timeout)`; that
   would be an undeclared behaviour change and `env` proxies bypass
   `resolve_to_addrs`). OpenAI config with `base_url: None` uses no pinning,
   as today. Then `genai::Client::builder().with_reqwest(hardened).build()`.
   Endpoint passed to `resolve_target` comes from `openai_endpoint_for(..)`
   (OpenAI) or the normalised Ollama base URL. Lazy
   `OnceCell::get_or_try_init` semantics: build exactly once, concurrent
   callers share.
3. `#[async_trait] impl LlmProvider for GenaiProvider` (annotate the struct
   `#[allow(dead_code)]` with the comment "unwired until task 1.2b flip;
   allow removed there"):
   - `id()`, `default_model()` as today; `supports_embed()` true.
   - `chat_stream(req)`: kind from config (OpenAI vs Ollama); model = request
     model or default; `ChatOptions` via `with_capture_usage(true)` and
     (when tools present) `with_capture_tool_calls(true)`; model arg =
     `resolve_target(kind, endpoint, api_key, model)`; map the genai stream:
     `ChatStreamEvent::Chunk` → `ChatEvent::Delta` (skip reasoning-only
     chunks); `ChatStreamEvent::ToolCallChunk` → IGNORED (never emitted);
     stream end → per-id-buffered complete tool calls emitted as
     `ChatEvent::ToolCall { id, name, arguments }` (one event per tool-call
     id, order preserved), then ONE `ChatEvent::Finished { usage:
     convert_usage(..), model: Some(requested model name), finish_reason:
     map_finish_reason(..), metadata: empty Map }`. Tool-call arguments: genai
     hands a `serde_json::Value` — pass `Value::String(s)` through as-is,
     otherwise `value.to_string()`. Mid-stream errors yield a
     mapped `Err` item (map_genai_error downcast path) and NO trailing
     `Finished`.
   - `embed(req)`: `resolve_target(kind, endpoint, key, embed_model_for(..))`;
     genai `embed_batch` call (the multi-input API; `Client::embed` is
     single-input only); output vectors PRESERVE genai response order (genai
     maps 1:1; no reordering logic anywhere); usage = `convert_embed_usage(..)`;
     build `EmbedResponse { embeddings, usage, model: Some(resolved embed
     model), metadata: empty Map }` (the field is `embeddings`, and `model`
     is load-bearing — producer.rs writes it to the `CAMEL_LLM_MODEL` header).
4. Compile + test: the new code must not affect the siumai path (the module
   is test-gated per Task 1.1 step 4, so production builds are untouched).

**Tests:** (command: `cargo test -p camel-component-llm --features openai,ollama genai_adapter`; expected: GREEN — the earlier unit tests still pass; the impl compiles)
- The Task 1.1 unit tests still pass unchanged (pure layer untouched).
- `genai_provider_implements_llm_provider`: construct `GenaiProvider::new(..)` inside the child tests module → assert `id()`, `default_model()`, `supports_embed()`; a `fn assert_send_sync<T: Send + Sync>()` style compile assertion on the concrete type.

**Acceptance:**
- `cargo check -p camel-component-llm --features openai,ollama` exits 0; default-feature tests green; `git status` shows ONLY `genai_adapter.rs`, `genai_adapter_tests.rs`, `mod.rs`, `Cargo.toml`, `Cargo.lock` touched.

- [x] 1.2a

### Task 1.2b: The flip — drop siumai, wire genai adapter, boundary switch

**Files:**
- `crates/components/camel-component-llm/Cargo.toml` (modified — remove the 4 siumai deps + fix the "ONLY files that import siumai" comment header to genai; features `openai = ["dep:genai"]`, `ollama = ["dep:genai"]`, `all-providers = ["openai","ollama"]`; add `wiremock = { workspace = true }` to `[dev-dependencies]`)
- `Cargo.lock` (modified — siumai family leaves the lockfile)
- `crates/components/camel-component-llm/src/provider/genai_adapter.rs` (modified — remove `#[allow(dead_code)]`, widen inner tests gate)
- `crates/components/camel-component-llm/src/provider/genai_adapter_tests.rs` (modified — gate widened + ported tests)
- `crates/components/camel-component-llm/src/provider/siumai_adapter.rs` (deleted)
- `crates/components/camel-component-llm/src/provider/siumai_adapter_tests.rs` (deleted)
- `crates/components/camel-component-llm/src/provider/mod.rs` (modified — `pub mod genai_adapter;` under the same cfg as before; doc comments say genai)
- `crates/components/camel-component-llm/src/provider_factory.rs` (modified — module path in call sites + doc lines only; `build_openai`/`build_ollama` signatures unchanged)
- `crates/components/camel-component-llm/src/lib.rs` (modified — doc comments only, lines mentioning siumai)
- `crates/components/camel-component-llm/tests/boundary.rs` (modified — allowlist + patterns)

**Steps:**
1. Cargo.toml flip per the Files list. `cargo tree -p camel-component-llm -e
   normal --no-default-features --features all-providers | grep siumai` must
   print nothing afterwards.
2. `build_openai`/`build_ollama` MOVE from the deleted `siumai_adapter.rs`
   into `genai_adapter.rs` (they construct `GenaiProvider`; signatures and
   SSRF validation calls unchanged). `provider_factory.rs` call sites update
   ONLY the module path (`crate::provider::genai_adapter::build_openai`) and
   doc lines — the factory itself must stay genai-free.
3. `mod.rs`/`lib.rs`: flip the module decl to production
   `#[cfg(any(feature = "openai", feature = "ollama", feature = "all-providers"))] pub mod genai_adapter;`
   and widen the inner tests gate to
   `#[cfg(all(test, any(feature = "openai", feature = "ollama")))]`; doc
   comments say genai (trait and types untouched).
4. Port still-relevant tests from the deleted `siumai_adapter_tests.rs` into
   `genai_adapter_tests.rs` (e.g. tool/message/usage conversion parity and
   error-mapping cases not already covered by Task 1.1), then delete both
   siumai files.
5. Widen the tests-file gate to
   `#[cfg(all(test, any(feature = "openai", feature = "ollama")))]` so
   Ollama-only builds run the adapter tests too (valid post-flip: both
   features enable `dep:genai`).
6. Remove the `#[allow(dead_code)]` from `GenaiProvider`.
7. `tests/boundary.rs`: scanned patterns become `genai::` and `use genai`
   (word-boundary matched so `genai_adapter` identifiers/paths do NOT match);
   allowlist narrows to `src/provider/genai_adapter.rs` and
   `src/provider/genai_adapter_tests.rs` ONLY (provider_factory.rs removed);
   doc comment cites the ADR-0020 amendment.

**Tests:** (command: `cargo test -p camel-component-llm --features all-providers`; expected: RED during the flip, GREEN at the end)
- All Task 1.1 unit tests still pass against real genai types.
- `build_openai_builds_genai_provider`: `OpenaiProviderConfig` → `build_openai` → provider with `id`/`default_model` set (no network at build time — lazy).
- `build_ollama_builds_genai_provider`: `OllamaProviderConfig` → same shape.
- Existing factory SSRF tests (`build_ollama_rejects_non_http_base_url`, `validate_llm_url_pinned_*`) pass UNCHANGED.

**Acceptance:**
- `cargo test -p camel-component-llm --features all-providers` green; `cargo test -p camel-component-llm` (mock default) green with zero changes to producer/mock/config/cost/cache files (`git status` must list only the files above).
- `cargo check -p camel-component-llm --features openai` and `--features ollama` each exit 0.
- No `siumai` token remains in `src/` (rg hit count 0).
- `cargo xtask lint-unwrap`, `cargo xtask lint-component-deps` pass.

- [x] 1.2b

### Task 1.3: Wiremock chat-stream acceptance suite, module `wiremock_chat` (OpenAI)

**Files:**
- `crates/components/camel-component-llm/src/provider/genai_adapter_tests.rs` (modified — append `mod wiremock_chat`)

**Steps:**
1. In `mod wiremock_chat`: helper that spawns a `wiremock::MockServer` and
   builds a `GenaiProvider` via `new_with_build_override` where the override
   returns `genai::Client::builder().with_reqwest(reqwest::Client::builder().no_proxy().build().map_err_to_llm()).build()`
   (build the client via the BUILDER — `Client::new().no_proxy()` does not
   compile; map the build error to `LlmError`) and the provider config's
   endpoint points at the mock server URL — OpenAI
   configs use `format!("{}/v1", server.uri())` (exercises trailing-slash
   normalisation), Ollama configs use `server.uri()` bare. Resolved through
   `resolve_target`; NO real network.
2. Canned OpenAI SSE responses via `wiremock::ResponseTemplate::new(200)`
   with header `content-type: text/event-stream` and raw body strings (lines
   like `data: {"choices":[{"delta":{"content":"hi"}}]}` terminated by
   `data: [DONE]`).
3. Write the tests below; each drives `chat_stream` to completion and asserts
   on the collected `ChatEvent` sequence.

**Tests:** (command: `cargo test -p camel-component-llm --features openai -- wiremock_chat`; expected: GREEN — impl exists; failures are real adapter bugs to fix here)
- `openai_stream_maps_deltas_usage_finish`: SSE with 3 text chunks + final usage chunk (`prompt_tokens: 11, completion_tokens: 7, total_tokens: 18`) + `finish_reason: "stop"` → `Delta`×3 in order then exactly one `Finished` with `usage = Some(LlmUsage { 11, 7, 18 })`, `finish_reason = Some(Stop)`, `model = Some(the requested model name)`.
- `openai_stream_request_contains_include_usage`: same mock captures the request → body contains `"stream_options":{"include_usage":true}`.
- `openai_reasoning_chunk_produces_no_delta`: SSE with a `reasoning_content` delta between text deltas → no `Delta` for it; text deltas unchanged.
- `openai_missing_completion_usage_is_none`: final chunk carries prompt tokens only → `Finished.usage` is `None`.
- `openai_tool_call_chunked_args_emitted_once_complete`: SSE tool call id `call_1`, name `get_weather`, arguments streamed as `{"loc` + `ation":"Paris"}` across deltas → exactly ONE `ToolCall` with complete arguments JSON `{"location":"Paris"}` before `Finished`; no `ToolCall` on partial deltas.
- `openai_two_parallel_tool_calls_two_events`: two distinct ids (`call_1`, `call_2`) each with chunked args → two `ToolCall` events, one per id, each complete, then one `Finished` with `finish_reason = Some(ToolCall)`.
- `openai_two_calls_in_one_delta_only_first_pinned`: SSE packing TWO tool calls inside a SINGLE delta WITHOUT any `finish_reason` in that delta; `finish_reason` and `[DONE]` arrive in a LATER chunk (genai's finish-reason path iterates all tool calls, so this separation is load-bearing). Doc-comment: pins the known genai general-path `delta_tool_calls.first()` limitation → exactly one `ToolCall` emitted.
- `openai_mid_stream_error_yields_err_no_finished`: mock answers 200 with `content-type: text/event-stream`, one valid delta then an invalid SSE payload (broken JSON after `data: `) → the stream yields `Delta` then an `Err` item, and NO `Finished` afterwards.
- `openai_config_ollama_style_model_hits_openai_route`: OpenAI config with model `qwen3.5:4b`, mock mounted ONLY on `POST /v1/chat/completions` (with a canned minimal SSE response) → the request reaches the OpenAI route and completes; proves end-to-end explicit targeting (the name-inference failure mode would hit a different route/adapter kind).

**Acceptance:**
- All 9 tests pass; request-capture proves `include_usage`.
- `cargo fmt --check` and `cargo clippy -p camel-component-llm --all-features --all-targets -- -D warnings` exit 0.

- [x] 1.3

### Task 1.4: Wiremock embed + Ollama native suites, modules `wiremock_embed` + `wiremock_ollama`

**Files:**
- `crates/components/camel-component-llm/src/provider/genai_adapter_tests.rs` (modified — append `mod wiremock_embed`, `mod wiremock_ollama`)

**Steps:**
1. Reuse the Task 1.3 helper pattern; `wiremock_embed` uses the OpenAI config
   (`/v1/embeddings` route), `wiremock_ollama` the Ollama config (`/api/embed`,
   `/api/chat` routes).
2. Ollama chat NDJSON body with header `content-type: application/x-ndjson`,
   TWO lines (usage arrives only on the done line — the Ollama streamer skips
   the usage block otherwise):
   `{"message":{"role":"assistant","content":"hi"},"done":false}`
   then `{"message":{"role":"assistant","content":""},"done":true,"done_reason":"stop","prompt_eval_count":9,"eval_count":5}`.
3. Embedding vectors carry per-vector identity markers (e.g. value patterns
   `[1.0, i, 0.5]` for input i) so identity is provable; mocks return vectors
   in REQUEST order (genai maps response order 1:1 — the test asserts the
   adapter neither drops nor reorders).
4. Write the tests below.

**Tests:** (command: `cargo test -p camel-component-llm --features openai,ollama -- wiremock_embed wiremock_ollama`; expected: GREEN — failures are real adapter bugs)
- `embed_openai_vectors_match_request_order`: `/v1/embeddings` returns marker-carrying vectors in request order → output vector i carries input i's marker (order + identity assertion); usage from the body's `usage.prompt_tokens` → `LlmUsage { prompt, 0, prompt }`.
- `embed_ollama_vectors_match_request_order`: `/api/embed` with multiple inputs → same order + identity assertion; usage from `prompt_eval_count` → total defaults to prompt.
- `embed_request_body_carries_model`: OpenAI mock asserts captured body `model == "embeddinggemma"` when the request has no explicit model (default applied) AND the returned `EmbedResponse.model` is `Some("embeddinggemma")`; second case with explicit model `text-embed-9` asserts body `model == "text-embed-9"` and `resp.model` matches.
- `ollama_chat_ndjson_usage_from_eval_counts`: NDJSON stream with `prompt_eval_count: 9`, `eval_count: 5` → `Finished.usage == Some(LlmUsage { 9, 5, 14 })`.
- `ollama_chat_missing_eval_count_usage_behaviour`: same two-line NDJSON shape but the done line carries `prompt_eval_count` only (no `eval_count`) → `Finished.usage` is `None` (pins the all-or-nothing chat usage rule for Ollama).

**Acceptance:**
- All 5 tests pass under `--features openai,ollama`; additionally `cargo test -p camel-component-llm --features ollama -- wiremock_ollama` passes (gate widened in 1.2b makes this run non-vacuous).
- `cargo fmt --check` + `cargo clippy -p camel-component-llm --all-features --all-targets -- -D warnings` exit 0.

- [x] 1.4

### Task 1.5: Wiremock error-mapping + SSRF + build-once suite, module `wiremock_error`

**Files:**
- `crates/components/camel-component-llm/src/provider/genai_adapter_tests.rs` (modified — append `mod wiremock_error` + SSRF/concurrency tests)

**Steps:**
1. Port the SSRF tests from the deleted file: `build_client_rejects_ssrf_link_local_base_url`
   (OpenAI, `http://169.254.169.254/`), `build_client_rejects_ollama_ssrf_link_local`
   (Ollama) — same arrange/act/assert against the genai adapter.
2. Port `client_built_once_under_concurrency` with the new seam: override
   returns a genai `Client` aimed at a wiremock endpoint, 8 concurrent first
   `embed` calls → build count exactly 1, all succeed.
3. Write the error-mapping tests below in `mod wiremock_error` (429/401/503
   via wiremock status + body; conn-refused via a closed local port; timeout
   via a delayed wiremock response, delay longer than the configured
   timeout).

**Tests:** (command: `cargo test -p camel-component-llm --features all-providers -- wiremock_error build_client client_built`; expected: GREEN)
- `chat_429_rate_limit_retryable_no_retry_after`: chat mock 429 + `{"error":{"message":"rate limit"}}` → stream `Err(LlmError::RateLimit { retry_after: None })`; `is_retryable` true.
- `embed_429_honours_delta_seconds_retry_after`: embed mock 429 + header `Retry-After: 7` + rate-limit body → `RateLimit { retry_after: Some(Duration::from_secs(7)) }`.
- `embed_429_http_date_retry_after_maps_to_none`: embed mock 429 + `Retry-After: Wed, 21 Oct 2026 07:28:00 GMT` → `retry_after: None`.
- `chat_429_insufficient_quota_not_retryable`: chat mock 429 + `{"error":{"code":"insufficient_quota","message":"billing"}}` → `QuotaExceeded`; `is_retryable` false.
- `embed_429_insufficient_quota_not_retryable`: same via embed path.
- `chat_401_maps_auth_failed`: 401 body → `AuthFailed`, not retryable.
- `chat_503_maps_provider_unavailable_retryable`: 503 → `ProviderUnavailable`, retryable.
- `chat_connection_refused_maps_network_retryable`: endpoint = closed local port → `Network`, retryable.
- `chat_timeout_maps_timeout_with_configured_duration`: the BuildOverride builds its reqwest client with `.no_proxy().timeout(100ms)`; wiremock delays 300ms; provider `configured_timeout` stays 1s → error is `Timeout(Duration::from_secs(1))`, proving the mapper uses the CONFIGURED value, not the transport one.
- `build_client_rejects_ssrf_link_local_base_url` (ported): err message contains "blocked".
- `build_client_rejects_ollama_ssrf_link_local` (ported): same for Ollama config.
- `client_built_once_under_concurrency` (ported): 8 racing first calls, `client_builds == 1`, all calls share the cached client and succeed.

**Acceptance:**
- All 12 tests pass; `cargo fmt --check` + clippy (as above) exit 0; `cargo xtask lint-test-sleep` and `cargo xtask lint-unbounded-wait` pass.

- [x] 1.5

### Task 1.6: Docs — CONTEXT.md term, CONTEXT-MAP, README, ADR-0020 amendment, smoke workflow comment

**Files:**
- `crates/components/camel-component-llm/CONTEXT.md` (modified — rename `SiumaiProvider` term to `GenaiProvider`, definition + _Avoid_ lines updated; L77 timeout note now references the genai mapping)
- `CONTEXT-MAP.md` (modified — ADR-0020 summary line + LlmProvider bullet: siumai → genai, `provider/genai_adapter.rs`)
- `crates/components/camel-component-llm/README.md` (modified — feature/provider table wording to genai)
- `docs/adr/0020-llm-component-provider-adapter-boundary.md` (modified — append `## Amendment 2026-10-19 (llm-genai-adapter)` section)
- `docs/src/components/llm.md` (modified — L71 area: `SiumaiProvider` → `GenaiProvider`, path → `provider/genai_adapter.rs`, siumai wording → genai)
- `docs/src/concepts/glossary.md` (modified — L92 area: siumai isolation wording → genai)
- `.github/workflows/integration-ollama-smoke.yml` (modified — header comment lines: "siumai adapter" → "genai adapter"; no job logic changes)

**Steps:**
1. ADR-0020 amendment text: boundary decision STANDS; the SDK behind it
   changes siumai 0.11.0-beta.9 → genai =0.6.5 (rationale: beta.10 removed
   the imported capability traits + the Ollama provider, MSRV 1.95 vs
   workspace 1.89; genai stable covers chat_stream + embed, ≈42 net-new unique
   packages over the mock baseline (siumai comparison figure was spike-era), accepts our hardened reqwest via `with_reqwest`); the boundary
   allowlist narrows to `provider/genai_adapter.rs` + adapter tests (factory
   removed — imports no SDK); boundary test now scans `genai::`/`use genai`.
2. CONTEXT.md: `GenaiProvider` term — "The adapter that bridges `LlmProvider`
   to the genai SDK. Lives in `provider/genai_adapter.rs`. If genai breaks,
   only this file changes." _Avoid_ line: OpenAiClient/OllamaClient are not
   exposed (genai types stay internal).
3. CONTEXT-MAP.md lines 70 + 165: siumai → genai wording, same structure.
4. README table: provider rows/features keep `openai`/`ollama`/`all-providers`
   names; descriptions say genai-backed.
5. Smoke workflow comment: mechanical siumai→genai wording swap in the header
   comment only.
6. Site docs: apply the same term/path swap in `docs/src/components/llm.md`
   and `docs/src/concepts/glossary.md` (Files list above).
7. Do NOT touch `docs/GLOSSARY.md` (does not exist in the repo).

**Tests:** (command: `grep -rin siumai CONTEXT-MAP.md docs/src/components/llm.md docs/src/concepts/glossary.md crates/components/camel-component-llm/README.md crates/components/camel-component-llm/CONTEXT.md crates/components/camel-component-llm/Cargo.toml .github/workflows/integration-ollama-smoke.yml crates/components/camel-component-llm/src docs/adr/0020-llm-component-provider-adapter-boundary.md | grep -vi "amendment\|superseded\|0.11.0-beta.9\|historical"`)
- Expected: zero remaining live references (only the ADR amendment's
  description of the superseded state may mention siumai).

**Acceptance:**
- The grep above exits clean; `cargo xtask lint-context-citations` passes; doc gate `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-component-llm --no-deps` exits 0.

- [x] 1.6

### Task 1.7: Boundary negative verification + local gate sweep

**Files:**
- `crates/components/camel-component-llm/src/producer.rs` (touched ONLY for the temporary boundary violation probe, reverted in-step)
- no other file changes; this task verifies and sweeps

**Steps:**
1. Probe: temporarily add `#[allow(unused_imports)] use genai::Client as _BoundaryProbe;`
   to `src/producer.rs` (the `genai::client::Client` path is private — the
   re-export compiles); run `cargo test -p camel-component-llm --test boundary --features all-providers`;
   confirm it FAILS on the boundary assertion (NOT a compile error — the
   import must compile for the scanner to catch it). Revert the probe;
   re-run; confirm green. Record both outcomes.
2. Run the full local gate set for the touched surface and fix anything this
   change caused: `cargo fmt --check --all`; `cargo clippy -p
   camel-component-llm --all-features --all-targets -- -D warnings`; `cargo
   xtask lint-unwrap`; `cargo xtask lint-secrets`; `cargo xtask
   lint-single-source`; `cargo xtask lint-non-exhaustive`; `cargo xtask
   lint-log-levels`; `cargo xtask lint-log-redaction`; `cargo xtask
   lint-cancel-tokens`; `cargo xtask lint-test-sleep`; `cargo xtask
   lint-unbounded-wait`; `cargo xtask lint-ignore`; `cargo xtask
   lint-component-deps`; `cargo xtask lint-gate-forwarding`; `cargo xtask
   lint-context-citations`; `cargo xtask lint-metric-labels`; `cargo xtask
   schema --check`; `cargo audit`; `RUSTDOCFLAGS="-D warnings" cargo doc -p
   camel-component-llm --no-deps`; `cargo test -p camel-component-llm
   --all-features`.
3. MSRV probe: `cargo +1.89 check -p camel-component-llm --features
   all-providers` if a 1.89 toolchain is installed; if rustup reports no
   1.89 toolchain, record `msrv: verify-in-spike-unknown` in the task report
   (do NOT silently skip).
4. Live-suite prep check (execution is a STAGE 4 gate): confirm
   `tests/ollama_live.rs` is untouched by the change (`git diff --stat` shows
   no ollama_live entry) and still compiles: `cargo test -p
   camel-component-llm --test ollama_live --features ollama --no-run`.

**Tests:**
- Boundary probe: expected FAIL with violation, GREEN after revert (both recorded).
- All gates in step 2: exit 0 each.

**Acceptance:**
- `git status` clean after the probe revert (only intended change files differ from main).
- Every gate exit code recorded in the task report; no unexplained failures.

- [x] 1.7
