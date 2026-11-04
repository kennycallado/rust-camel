# Design: llm-decisions

## Approach

### Decision seam (ADR-0020 addendum)

New module `src/decision/` in `camel-component-llm`. It has no genai imports, and
the existing `tests/boundary.rs` import scan (which already covers all of `src/`)
enforces that. Doc comments may mention genai.

Feature gating:

- `decision/openai.rs`, `decision/system_one.rs`, and their tests use
  `#[cfg(feature = "decisions")]`.
- `provider/http_errors.rs` uses `#[cfg(any(feature = "decisions", feature =
  "openai", feature = "ollama", feature = "generic-providers", feature =
  "all-providers"))]`. A mock-only build therefore has no dead code.
- `mod.rs`, `dup_map.rs`, `validate.rs`, `mock.rs`, and `producer.rs` are always
  compiled.

`decision/mod.rs` holds the trait and the Camel-owned types. All public enums are
`#[non_exhaustive]`. The types use named fields, not tuples (ADR-0091).

```rust
#[async_trait]
pub trait DecisionProvider: Send + Sync {
    fn id(&self) -> &str;
    fn default_model(&self) -> &str;
    fn capabilities(&self) -> DecisionCapabilities;           // { max_questions: usize }
    async fn decide(&self, req: DecisionRequest) -> Result<DecisionResponse, LlmError>;
}
pub struct DecisionRequest { pub model: String, pub state: DecisionState,
                             pub questions: Arc<[DecisionQuestion]> }
pub enum DecisionState { Text(String), Json(serde_json::Value) }
pub struct DecisionQuestion { pub name: String, pub instructions: String, pub kind: QuestionKind }
pub enum QuestionKind { Predicate, Choice { choices: Vec<ChoiceOption> }, Score { levels: Vec<ScoreLevel> } }
pub struct ChoiceOption { pub value: String, pub description: Option<String> }
pub struct ScoreLevel  { pub label: String, pub description: Option<String> }
pub struct DecisionResponse { pub model: String, pub answers: Vec<NamedAnswer>, pub usage: DecisionUsage }
pub struct NamedAnswer { pub name: String, pub answer: DecisionAnswer }
pub enum DecisionAnswer {
    Predicate { probability: f64 },
    Choice { choice: String, confidence: f64, probabilities: Vec<ChoiceProbability> }, // declared order
    Score  { score: f64, confidence: f64, probabilities: Vec<ScoreProbability> },     // level order
    Refusal,
}
pub struct ChoiceProbability { pub value: String, pub probability: f64 }
pub struct ScoreProbability  { pub level: usize, pub probability: f64 }
pub struct DecisionUsage { pub input_tokens: Option<u64>, pub output_tokens: Option<u64>,
                           pub cached_input_tokens: Option<u64>, pub cache_write_input_tokens: Option<u64> }
```

Thresholds are not part of the provider request. They belong to the question-set
config and the producer. The adapters reuse `LlmError`, so `is_retryable`, the
ADR-0021 retry loop, and the `CamelError` mapping stay shared. `LlmProvider` does
not change.

**Genai migration path:** when a released genai version supports decisions, a genai
decision adapter implements `DecisionProvider` in an allowlisted genai file. The
config `type` values stay the same. Only the factory dispatch changes. ADR-0020 gets
an addendum that records this trigger.

### Configuration (TOML)

```toml
default_decision_provider = "local"

[decision_providers.oai]
type = "openai"             # api_key, default_model required; base_url default https://api.openai.com/v1
api_key = "sk-..."
default_model = "gpt-6-luna"

[decision_providers.ts]
type = "typesafe"           # api_key, default_model required; base_url default https://api.typesafe.ai
api_key = "ts-..."
default_model = "jev-latest"

[decision_providers.local]
type = "ollama"             # base_url, default_model required; no api_key
base_url = "http://127.0.0.1:11434"
default_model = "tev1:0.8b"

[decision_providers.fake]
type = "mock"               # all optional, see Mock
predicate_probability = 1.0
refuse = ["dept"]

[[decision_question_sets.triage.questions]]
name = "is_billing"
type = "predicate"
instructions = "Is this a billing issue?"
threshold = 0.8

[[decision_question_sets.triage.questions]]
name = "dept"
type = "choice"
instructions = "Which department?"
choices = [{ value = "billing", description = "payments" }, { value = "tech" }]
min_confidence = 0.7

[[decision_question_sets.triage.questions]]
name = "urgency"
type = "score"
instructions = "How urgent?"
levels = [{ label = "low" }, { label = "high", description = "outage" }]
```

The `openai`, `typesafe`, and `ollama` types share these optional fields:
`timeout_secs`, `max_concurrency`, and `network_retry` (`NetworkRetryPolicy`). These
types have no `pricing`, cache, or `api_key` (Ollama) fields. `deny_unknown_fields`
applies to the decision structs. Question-set `questions` is an ordered array, so
answer order is defined. `Debug` redacts `api_key`.

`LlmGlobalConfig::validate()` checks the question sets:

- 1–400 questions. Names match `^[A-Za-z0-9_-]{1,64}$` and are unique.
- `instructions` is not blank.
- A choice has 2–255 unique, non-blank values.
- A score has 2–10 levels with non-blank labels.
- `threshold` is valid on a predicate only. `min_confidence` is valid on a choice or
  score only. Both are finite values in `[0, 1]`.
- API keys, base URLs, and default models are not blank.
- `predicate_probability` is finite and in `[0, 1]`.
- Every `refuse` entry is a syntactically valid question name.
- `timeout_secs` and `max_concurrency` are not 0.

The question config struct (`QuestionConfig`) is flat:

- `name`, `type` (`predicate` | `choice` | `score`), `instructions`.
- Optional `choices`, `levels`, `threshold`, and `min_confidence`.

`deny_unknown_fields` is set, so a misplaced field (for example, `threshold` on a
choice) deserializes and then fails in `validate()` with `CamelError::Config`.
`validate()` also rejects `choices` on a non-choice question and `levels` on a
non-score question.

`LlmComponent::new` now calls `config.validate()` before it builds the maps, as
`LlmBundle` already does.

### Mock

`decision/mock.rs` sets these fields:

- `default_model`: default `"mock-decision"`.
- `predicate_probability`: default `1.0`.
- `refuse`: `Vec<String>`, default empty.
- `error_message`: `Option<String>`.
- `max_questions`: 400.

The answers are deterministic:

- `error_message` set → `Err(LlmError::provider(msg))`.
- A question named in `refuse` → `Refusal`.
- Predicate → `predicate_probability`.
- Choice → the first declared value. It gets probability 1.0 and every other value
  gets 0.0. Confidence 1.0.
- Score → `score = 0.0`. Level 0 gets probability 1.0 and every other level 0.0.
  Confidence 1.0.
- Usage: all four fields `None`.

### HTTP adapters (feature `decisions`)

| `type` | Codec file | URL | Auth | max_questions |
|---|---|---|---|---|
| `openai` | `decision/openai.rs` | `{base}/decisions` | Bearer | 400 |
| `typesafe` | `decision/system_one.rs` | `{base}/v1/systemone` | Bearer | 400 (project cap; TypeSafe documents only ≥1) |
| `ollama` | `decision/system_one.rs` | `{base}/v1/systemone` | none | 64 (live-probed) |

URL join: trim the trailing `/` characters from `base`, then append the path.
Requests are built with `serde_json::to_vec` plus `.body()`. Responses are read with
`.bytes()` plus `serde_json::from_slice`. The workspace reqwest has no `json`
feature, and this change adds none.

**SSRF and construction** mirror `GenaiProvider`:

- At startup, `build_decision_provider_map` calls `validate_llm_url(base_url, policy)`
  for every explicit or required `base_url`. That is the HTTPS rule and blocked-IP
  check under `PublicHttpsOnly`.
- The HTTP client is built lazily on the first `decide`, in a `tokio::sync::OnceCell`.
  Before the build, `validate_llm_url_pinned` runs in `spawn_blocking`. The DNS pin
  applies only for an explicit `base_url` with a domain host.
- Default public URLs (no `base_url`) are not pinned. This matches chat.
- Ollama on loopback needs `allow_internal = true`.

**Timeouts:** the same as chat.

- Transport timeout per attempt: the request is sent with `.timeout(configured)`,
  where `configured = timeout_secs.or(global).unwrap_or(30)`. A reqwest timeout maps to
  `LlmError::Timeout(configured)`.
- Total deadline: the producer wraps the whole retry loop once in `apply_timeout`.
- These are two different scopes (per attempt and total). They are not nested
  producer timeouts. ADR-0021 forbids only nested producer timeouts.

**Status mapping:** `provider/http_errors.rs` (no SDK import) receives
`map_status`, `is_insufficient_quota`, `parse_retry_after`, and `truncate_body`,
moved without changes from `genai_adapter.rs`. The genai adapter imports them, so
its mapping does not change. The decision adapters handle 413 and 415 →
`InvalidRequest` before they call `map_status`. 529 goes to the existing 5xx arm.
Error text keeps the existing 200-byte truncated body and never includes headers.

**Client:** the decision adapters build their `reqwest::Client` only through
`crate::hardened_http_client_with_pinning(pinning)`. That client follows no
redirects, uses `no_proxy`, and applies `resolve_to_addrs`. The per-request
`.timeout(configured)` overrides the client's 30 s default. A 3xx response is
non-2xx and goes through `map_status` (the `_` arm → `Provider`). It is never
followed. A response body is read chunk by chunk with `Response::chunk()`, up to
`MAX_DECISION_RESPONSE_BYTES = 8 MiB`; a larger body → `Protocol`. The decision
path passes the whole capped body to `map_status`, because the
`insufficient_quota` check parses it as JSON and a real quota body is longer than
200 bytes. Every `map_status` arm truncates what it puts in the error text, and
413/415 bodies go through `truncate_body`. Every error text therefore holds at
most 200 body bytes, and genai stays unchanged. (Amended 2026-11-03 during Task
2.2. The original wording truncated before `map_status`, which turned long quota
bodies into retryable `RateLimit`.)

**Codecs:**

- OpenAI request: `{model, input, questions:[{type, name, instructions,
  choices:[{value, description?}] | levels:[{label, description?}]}]}`.
  - `Text` → `input` string.
  - A `Json` object, or a `Json` array of objects → `input:[{role:"user",
    content:[{type:"input_json", data:<the whole value>}]}]`. It is one part, and
    `data` holds the whole object or array.
- OpenAI response, as the official reference documents it:
  ```json
  {"model":"gpt-6-luna","answers":[
    {"type":"predicate","name":"is_billing","probability":0.97},
    {"type":"choice","name":"dept","choice":"billing","confidence":0.99,
     "probabilities":[{"value":"billing","probability":0.99},{"value":"tech","probability":0.01}]},
    {"type":"score","name":"urgency","score":0.96,"confidence":0.17,
     "probabilities":[{"label":"low","value":0,"probability":0.29},{"label":"high","value":1,"probability":0.71}]},
    {"type":"refusal","name":"other"}],
   "usage":{"input_tokens":2000,"input_tokens_details":{"cached_tokens":1500,"cache_write_tokens":0},
            "output_tokens":0,"output_tokens_details":{"reasoning_tokens":0},"total_tokens":2000}}
  ```
- System One response, from the live Ollama `tev1:0.8b` probe and the TypeSafe
  reference. `confidence` is a wire field on choice and score, so nothing is derived.
  ```json
  {"model":"tev1:0.8b","answers":{
    "is_billing":{"type":"noul","noul":0.97},
    "dept":{"type":"choice","choice":"billing","confidence":0.99,
            "probabilities":{"billing":0.99,"tech":0.01}},
    "urgency":{"type":"score","score":0.96,"confidence":0.17,"legend":{"0":"low","1":"high"},
               "probabilities":{"0":0.29,"1":0.71}}},
   "usage":{"input_tokens":375,"output_tokens":0}}
  ```
  A missing `confidence` on a choice or score → `Protocol`.
- OpenAI response:
  - `answers[]` are correlated by `name`.
  - A duplicate `name` → `Protocol`.
  - A duplicate `value` in `probabilities[]` → `Protocol`.
  - Score `value` is the level index. The wire `label` is ignored, and the declared
    label is used.
  - `refusal` → `Refusal`.
  - Usage comes from `input_tokens`, `input_tokens_details.{cached_tokens,
    cache_write_tokens}`, and `output_tokens`.
- System One request: `{model, state, questions:{name:{type, instructions,
  criteria?}}}`.
  - Predicate → `noul`, no criteria.
  - Choice → an object `{value: description|null}`.
  - Score → an array of `"label: description"`, or `label` alone.
- System One response: `answers` and every `probabilities` object are decoded with a
  duplicate-rejecting map visitor (`decision/dup_map.rs`, `DupCheckedMap<V>`). A
  duplicate key → `Protocol`. Score keys `"0".."n-1"` are parsed as `usize`. `legend`
  is ignored. No refusal exists.

**Shared validation** (`decision/validate.rs`, `correlate`):

- One answer per question. A missing or unknown name → `Protocol`.
- A kind mismatch → `Protocol`.
- Every probability and confidence is finite and in `[0, 1]` (tolerance `1e-9`).
  Values are preserved, not clamped.
- A choice must be a declared value. Its distribution keys must equal the declared
  set.
- A score must be in `[0, levels-1]`. Its levels must equal `0..levels`.
- A distribution sum must be within `0.01` of 1.

### State mapping (producer, before I/O)

| Body | State |
|---|---|
| `Text`, `Xml`, UTF-8 `Bytes`, `Json(String)` | `Text` |
| `Json` object, `Json` array | `Json` |
| `Json` number, bool, or null; `Empty`; `Stream`; invalid UTF-8 | `InvalidRequest` |

The size is the text byte length or the serialized JSON byte length. It must not
exceed `max_prompt_bytes`. The OpenAI adapter also rejects a `Json` array that has a
non-object element, with `InvalidRequest` before I/O.

### Endpoint and producer

- `LlmOperation` gains `Decide` and becomes `#[non_exhaustive]`. `LlmGlobalConfig`
  and `LlmEndpointConfig` gain public fields, which breaks struct literals. These are
  pre-1.0 breaks, recorded in the crate `CONTEXT.md` "Breaking changes".
- `LlmEndpointConfig` gains `questions: Option<String>`.
- `from_uri("llm:decide?…")` rejects the parameters `stream`, `temperature`,
  `max_tokens`, and `system_prompt` with `InvalidUri`. `chat` and `embed` reject
  `questions`.
- `create_endpoint` resolves the decision provider (`provider` param or
  `default_decision_provider`) and the question set. It checks the set against
  `max_questions`. Each failure is `InvalidUri` with no I/O.
- The metadata descriptor gains `questions` and the description "LLM chat / embed /
  decide producer". The parity test is updated.

`DecisionProducer` (`decision/producer.rs`):

- `apply_timeout` and `run_with_retry` become free `pub(crate)` functions in
  `producer.rs`. `LlmProducer` calls them, and so does the decision producer.
- The semaphore is per producer, from `max_concurrency`, the same as chat.
- The body becomes
  `{"model", "answers": {name: answer}}`. Each answer is one of these shapes:
  - `{"type":"predicate","probability"}`
  - `{"type":"choice","choice","confidence","probabilities":{value:p}}`
  - `{"type":"score","score","confidence","probabilities":[{"level","label","probability"}]}`
  - `{"type":"refusal"}`
- `meets_threshold` (`probability >= threshold`) is added when a threshold is
  configured. `meets_min_confidence` is added when a minimum is configured. A
  refusal has neither, so `== true` routes fall through to `otherwise`. The docs
  warn about this.
- Headers: `CamelLlmProvider`, `CamelLlmModel`, `CamelLlmUsageAvailable=true`.
  `CamelLlmTokensIn` and `CamelLlmTokensOut` are set when reported.
  `CamelLlmCachedTokensIn` and `CamelLlmCacheWriteTokensIn` are new constants,
  also set when reported. There is no cost header.
- Dropping the producer future drops the request and the semaphore permit.

`LlmComponent` holds a `DecisionProviderMap` next to `ProviderMap`. `new()` and
`LlmBundle` build both. `from_parts` keeps its signature and has an empty decision
map (documented). `with_decision_providers(map)` adds the map.

## Affected crates

- camel-component-llm: decision module, config, endpoint, metadata, headers, the
  `http_errors` extraction, the producer helper extraction, tests, and `CONTEXT.md`.
- docs: `docs/src/components/llm.md`, `CONTEXT-MAP.md`, ADR-0020 addendum.

## Architecture boundaries

Components tier only:

- ADR-0032: question sets and providers are trusted operator config. The Exchange
  body is only the size-bounded state and never selects a provider, a set, or a
  limit.
- ADR-0020: there are no genai imports.
- ADR-0021: one manual retry loop and one total deadline.
- ADR-0033: HTTPS and SSRF validation at startup, and rejection of blank
  credentials.
- ADR-0092: Simple routes use the `meets_*` booleans. Float comparisons are tracked
  separately in rc-8ww5h.

## Phases

### Phase 1: contract, config, mock, and route surface
- **Goal:** `llm:decide` works end to end with the mock provider.
- **Dependencies:** none.
- **Externally-visible types/interfaces:** decision types and trait, decision config
  types, `LlmOperation::Decide`, `questions` URI param, new header constants.
- **Deliverable:** decision module (types, `dup_map`, `validate`, `mock`, producer),
  config validation, endpoint wiring, and a route test.
- **Exit-criteria:** `cargo test -p camel-component-llm` is green with the default
  features.

### Phase 2: HTTP adapters and docs
- **Goal:** OpenAI, TypeSafe, and Ollama adapters with contract tests and docs.
- **Dependencies:** Phase 1.
- **Externally-visible types/interfaces:** `decisions` feature, HTTP provider types.
- **Deliverable:** `http_errors.rs`, both codecs, wiremock tests, the ignored live
  Ollama test, docs, and the ADR addendum.
- **Exit-criteria:** `cargo test -p camel-component-llm --features all-providers`
  is green. A `--no-default-features --features mock` build is green.

## Alternatives considered

- **`decide()` on `LlmProvider`.** Every chat adapter would need a stub, and the
  operation shares no streaming semantics with chat.
- **Questions in headers.** Under ADR-0032 the headers are untrusted, and the
  endpoint-time capability checks would become runtime checks. Deferred.
- **One `system_one` type with a URL switch.** The key and cap rules differ, so
  explicit types keep validation simple.
- **Wait for genai.** No released version supports decisions. The trait seam makes
  the later swap local.
