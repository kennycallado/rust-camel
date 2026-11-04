# Tasks: llm-decisions

Crate root for all paths below: `crates/components/camel-component-llm/` (abbreviated
`LLM/`). Run every command in the feature worktree, never in the main checkout.
Every task ends with these commands, and both must exit 0:

- `cargo fmt --check -p camel-component-llm`
- `cargo clippy -p camel-component-llm --all-targets -- -D warnings`

From Task 2.5 on, every task also runs this command:

- `cargo clippy -p camel-component-llm --all-targets --features all-providers -- -D warnings`

Style norms (ADR-0091, new code only):

- Qualify free-function calls with their module.
- Import types unqualified.
- One module per `use`.
- Use named fields.
- Use `expect("reason")`, never `unwrap()`, outside tests.
- Put `#[non_exhaustive]` on new public enums.

Use no `tokio::time::sleep` in a test body. Wait with
`tokio::time::timeout(...)` or `MockEndpoint::await_exchanges(n, timeout)` instead.

## Shared test fixtures (canonical; every task uses exactly these)

"Triage" always means this 3-question set. It has 3 score levels, which differs
from the 2-level design.md example:

```toml
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

[[decision_question_sets.triage.questions]]
name = "urgency"
type = "score"
instructions = "How urgent?"
levels = [{ label = "low" }, { label = "medium" }, { label = "high", description = "outage" }]
```

The fixtures are these values:

- `OPENAI_TRIAGE_RESPONSE`:
  ```json
  {"model":"gpt-6-luna","answers":[
   {"type":"predicate","name":"is_billing","probability":0.97},
   {"type":"choice","name":"dept","choice":"billing","confidence":0.99,
    "probabilities":[{"value":"billing","probability":0.99},{"value":"tech","probability":0.01}]},
   {"type":"score","name":"urgency","score":0.96,"confidence":0.17,
    "probabilities":[{"label":"low","value":0,"probability":0.29},{"label":"medium","value":1,"probability":0.45},{"label":"high","value":2,"probability":0.26}]}],
   "usage":{"input_tokens":2000,"input_tokens_details":{"cached_tokens":1500,"cache_write_tokens":0},"output_tokens":0,"total_tokens":2000}}
  ```
- `SYSTEM_ONE_TRIAGE_RESPONSE`:
  ```json
  {"model":"tev1:0.8b","answers":{
   "is_billing":{"type":"noul","noul":0.97},
   "dept":{"type":"choice","choice":"billing","confidence":0.99,"probabilities":{"billing":0.99,"tech":0.01}},
   "urgency":{"type":"score","score":0.96,"confidence":0.17,"legend":{"0":"low","1":"medium","2":"high"},
              "probabilities":{"0":0.29,"1":0.45,"2":0.26}}},
   "usage":{"input_tokens":375,"output_tokens":0}}
  ```

Where they live:

- In-crate: Task 1.2 creates `#[cfg(test)] pub(crate) mod test_support` in
  `decision/mod.rs`. It holds `pub(crate) fn triage_questions() ->
  Arc<[DecisionQuestion]>` (the set above), `pub(crate) const TRIAGE_TOML: &str`
  (the TOML above, verbatim), and the two response consts, written as raw strings.
  `TRIAGE_TOML` contains only the question set; a test prepends its own provider
  tables.
- Integration tests: Task 1.9b creates `LLM/tests/common/decision.rs` with the same
  `TRIAGE_TOML` and response consts, included with `pub mod decision;` in
  `tests/common/mod.rs`. An integration test that needs them declares `mod common;`.

Producer construction in integration tests uses the pattern from
`tests/chat_materialized.rs:18-28`:
`component.create_endpoint(uri, &NoOpComponentContext)`, then
`endpoint.create_producer(Arc::new(NoopRuntimeObservability),
&common::make_producer_ctx())`, then `producer.call(exchange)` (`tower::Service`).

`NetworkRetryPolicy` TOML keys are `enabled`, `max_attempts`, `initial_delay_ms`
(integer), `multiplier`, `max_delay_ms` (integer), and `jitter_factor`.


## Phase 1: contract, config, mock, and route surface

### Task 1.1: Extract the producer timeout and retry helpers as free functions

**Files:**
- `LLM/src/producer.rs` (modified)

**Steps:**
1. Move the body of `LlmProducer::apply_timeout` into a module-level function
   `pub(crate) async fn apply_timeout<F, T>(timeout: Option<Duration>, fut: F) -> Result<T, LlmError>`
   with the same bounds and logic. Make the method a one-line delegate:
   `crate::producer::apply_timeout(self.timeout, fut).await`.
2. Move `LlmProducer::run_with_retry` to a module-level function
   `pub(crate) async fn run_with_retry<F, Fut, T>(semaphore, retry, content_started, route_id, op)`
   with the same signature, minus `Self`. Change every call site,
   `Self::run_with_retry(...)`, to `crate::producer::run_with_retry(...)`. The call sites are in
   `handle_chat_materialized` (about `producer.rs:803`) and `handle_embed` (about
   `producer.rs:891`). `apply_timeout` is called right after each of them.
3. Keep the doc comments on the new functions. Change no behavior.

**Tests:**
- No new tests. The existing `producer_timeout_tests`, `producer_retry_tests`, and
  `producer_semaphore_tests` are the regression net.
  - command: `cargo test -p camel-component-llm --lib`
  - expected: pass before and after the change

**Acceptance:**
- `cargo test -p camel-component-llm --lib` passes.
- `producer.rs` has exactly one module-level `pub(crate) async fn run_with_retry` and
  one module-level `pub(crate) async fn apply_timeout` (outside `impl LlmProducer`).

- [x] 1.1

### Task 1.2: Decision contract types and header constants

**Files:**
- `LLM/src/decision/mod.rs` (new)
- `LLM/src/lib.rs` (modified)
- `LLM/src/headers.rs` (modified)

**Steps:**
1. Create `decision/mod.rs` with the module doc
   `//! Camel-owned decision contract (ADR-0020 addendum). No genai imports.`
2. Define these items exactly as in design.md "Decision seam":
   - the trait `DecisionProvider` (`#[async_trait]`, `Send + Sync`), with `id`,
     `default_model`, `capabilities`, and `decide`
   - `DecisionCapabilities { pub max_questions: usize }` (`Debug, Clone, Copy,
     PartialEq, Eq`)
   - `DecisionRequest`, `DecisionState`, `DecisionQuestion`, `QuestionKind`,
     `ChoiceOption`, `ScoreLevel`, `DecisionResponse`, `NamedAnswer`,
     `DecisionAnswer`, `ChoiceProbability`, `ScoreProbability`, and `DecisionUsage`
3. Derive `Debug, Clone, PartialEq` on every data type. `DecisionUsage` also derives
   `Default, Copy, Eq`.
4. Derive `serde::Deserialize` with `#[serde(deny_unknown_fields)]` on `ChoiceOption`
   and `ScoreLevel`. `description` is `#[serde(default)]`.
5. Put `#[non_exhaustive]` on `DecisionState`, `QuestionKind`, and `DecisionAnswer`.
6. In `lib.rs`, add `pub mod decision;`, then re-export `DecisionProvider`,
   `DecisionRequest`, `DecisionResponse`, `DecisionAnswer`, and `DecisionState`.
7. In `headers.rs`, add `pub const CAMEL_LLM_CACHED_TOKENS_IN: &str =
   "CamelLlmCachedTokensIn";` and `pub const CAMEL_LLM_CACHE_WRITE_TOKENS_IN: &str =
   "CamelLlmCacheWriteTokensIn";`. Add both to the existing
   `header_constants_are_prefixed` test.
8. Add `#[cfg(test)] #[allow(dead_code)] pub(crate) mod test_support` to
   `decision/mod.rs`. It holds `triage_questions()`, `TRIAGE_TOML`,
   `OPENAI_TRIAGE_RESPONSE`, and `SYSTEM_ONE_TRIAGE_RESPONSE`, exactly as in "Shared
   test fixtures". `triage_questions()` builds the `DecisionQuestion` values in code.

**Tests:**
- `decision_usage_default_is_all_none`
  - setup: none
  - action: `DecisionUsage::default()`
  - assert: all four fields `== None`
  - command: `cargo test -p camel-component-llm --lib decision::tests`
  - expected: fails to compile before, passes after
- `triage_fixture_shape`
  - action: `test_support::triage_questions()`
  - assert: length 3, names `[is_billing, dept, urgency]`, and `urgency` is `Score` with
    3 levels
- `choice_option_rejects_unknown_field`
  - action: `toml::from_str::<ChoiceOption>("value = \"a\"\nweight = 1")`
  - assert: `is_err()`
- `header_constants_are_prefixed` (extended)
  - assert: the two new constants equal their literal strings

**Acceptance:**
- `cargo test -p camel-component-llm --lib` passes.
- `grep -rn "genai" LLM/src/decision/mod.rs` matches only comment lines.
- `cargo test -p camel-component-llm --test boundary` passes.

- [x] 1.2

### Task 1.3: Duplicate-rejecting JSON map decoder

**Files:**
- `LLM/Cargo.toml` (modified: add `decisions = []` to `[features]`, but not to `all-providers` yet)
- `LLM/src/decision/dup_map.rs` (new)
- `LLM/src/decision/mod.rs` (modified: add `pub(crate) mod dup_map;`)

**Steps:**
0. In `LLM/Cargo.toml` `[features]`, add `decisions = []`. The HTTP adapters land in Phase 2, and Task 2.5 adds the feature to `all-providers`. Declaring it now keeps the `cfg(feature = "decisions")` attributes free of `unexpected_cfgs` warnings.
1. Define `pub(crate) struct DupCheckedMap<V>(pub(crate) Vec<DupEntry<V>>)`, where
   `pub(crate) struct DupEntry<V> { pub(crate) key: String, pub(crate) value: V }`.
2. Implement `serde::Deserialize` for `DupCheckedMap<V>` where `V: Deserialize`.
   Use a `serde::de::Visitor` with `visit_map`. Collect the entries in wire order,
   and track the keys seen in a `HashSet<String>`. On a repeated key, return
   `serde::de::Error::custom(format!("duplicate key '{key}'"))`.
   `expecting` returns `"a JSON object with unique keys"`.

**Tests:** (module `#[cfg(test)] mod tests` in `dup_map.rs`)
- `dup_map_preserves_wire_order`
  - action: `serde_json::from_str::<DupCheckedMap<f64>>(r#"{"b":1.0,"a":2.0}"#)`
  - assert: keys are `["b","a"]`, values are `[1.0,2.0]`
  - command: `cargo test -p camel-component-llm --lib decision::dup_map`
  - expected: fails before, passes after
- `dup_map_rejects_duplicate_key`
  - action: decode `r#"{"a":1.0,"a":2.0}"#`
  - assert: `is_err()`, and the error text contains `duplicate key 'a'`
- `dup_map_rejects_non_object`
  - action: decode `"[1,2]"`
  - assert: `is_err()`

**Acceptance:**
- All three tests pass.
- Clippy is clean, with no `dead_code` warning. Declare the module in
  `decision/mod.rs` as `#[cfg_attr(not(feature = "decisions"), allow(dead_code))]
  pub(crate) mod dup_map;`. Only the System One codec (Task 2.4) uses it. Under the
  `decisions` feature it is used from Task 2.4 on. (Task 2.5 replaced this with
  `#[cfg(any(feature = "decisions", test))]` once the codecs had callers.)

- [x] 1.3

### Task 1.4a: Decision configuration types

**Files:**
- `LLM/src/decision/config.rs` (new)
- `LLM/src/decision/mod.rs` (modified: add `pub mod config;`)
- `LLM/src/config.rs` (modified)
- `LLM/src/config_tests.rs` (modified)
- `LLM/src/decision/config_tests.rs` (new; included from `decision/config.rs` with `#[cfg(test)] #[path = "config_tests.rs"] mod tests;`)

**Steps:**
1. In `decision/config.rs`, define these types (all `Clone, serde::Deserialize`. Put
   `#[serde(deny_unknown_fields)]` on the structs only, not on the tagged enum
   `DecisionProviderConfig` or on `QuestionType`):
   - `OpenaiDecisionConfig { pub api_key: String, pub default_model: String,
     #[serde(default)] pub base_url: Option<String>, #[serde(default)] pub
     timeout_secs: Option<u64>, #[serde(default)] pub max_concurrency: Option<usize>,
     #[serde(default)] pub network_retry: Option<NetworkRetryPolicy> }`
   - `TypesafeDecisionConfig`: the same fields as `OpenaiDecisionConfig`.
   - `OllamaDecisionConfig { pub base_url: String, pub default_model: String, pub
     timeout_secs, pub max_concurrency, pub network_retry }`, using the same types and
     defaults as above.
   - `MockDecisionConfig { #[serde(default = "default_mock_decision_model")] pub
     default_model: String, #[serde(default = "default_predicate_probability")] pub
     predicate_probability: f64, #[serde(default)] pub refuse: Vec<String>,
     #[serde(default)] pub error_message: Option<String> }`. The defaults are
     `"mock-decision"` and `1.0`.
   - `#[serde(tag = "type", rename_all = "lowercase")] #[non_exhaustive] pub enum
     DecisionProviderConfig { Openai(OpenaiDecisionConfig),
     Typesafe(TypesafeDecisionConfig), Ollama(OllamaDecisionConfig),
     Mock(MockDecisionConfig) }`
   - `#[serde(rename_all = "lowercase")] #[non_exhaustive] pub enum QuestionType {
     Predicate, Choice, Score }` (also derive `Debug, Copy, PartialEq, Eq`)
   - `QuestionConfig { pub name: String, #[serde(rename = "type")] pub kind:
     QuestionType, pub instructions: String, #[serde(default)] pub choices:
     Option<Vec<ChoiceOption>>, #[serde(default)] pub levels: Option<Vec<ScoreLevel>>,
     #[serde(default)] pub threshold: Option<f64>, #[serde(default)] pub
     min_confidence: Option<f64> }`
   - `QuestionSetConfig { pub questions: Vec<QuestionConfig> }`
2. Write a manual `impl fmt::Debug` for `OpenaiDecisionConfig`,
   `TypesafeDecisionConfig`, and `DecisionProviderConfig`. Each prints `api_key` as
   `"[REDACTED]"` and `base_url` through `camel_api::redact::redact_url`. Derive
   `Debug` on the other types.
3. Add accessors on `DecisionProviderConfig`:
   - `pub fn timeout_secs(&self) -> Option<u64>`,
     `pub fn max_concurrency(&self) -> Option<usize>`, and
     `pub fn network_retry(&self) -> Option<NetworkRetryPolicy>`. Mock returns `None`.
   - `pub fn type_name(&self) -> &'static str`, which returns
     `"openai" | "typesafe" | "ollama" | "mock"`.
4. Add `pub fn to_questions(&self) -> Arc<[DecisionQuestion]>` on
   `QuestionSetConfig`. It maps each `QuestionConfig` in order:
   - `Predicate` → `QuestionKind::Predicate`.
   - `Choice` → `QuestionKind::Choice { choices: choices.clone().unwrap_or_default() }`.
   - `Score` → `QuestionKind::Score { levels: levels.clone().unwrap_or_default() }`.
   Callers use it only after validation (Task 1.4b).
5. In `config.rs`, add these fields to `LlmGlobalConfig` (`#[serde(default)]` on each)
   and to its `Default` impl:
   - `pub default_decision_provider: Option<String>`
   - `pub decision_providers: HashMap<String, DecisionProviderConfig>`
   - `pub decision_question_sets: HashMap<String, QuestionSetConfig>`
6. In `LLM/src/config_tests.rs`, append `..Default::default()` to every full
   `LlmGlobalConfig { .. }` struct literal, so that they compile. There are 10 of them, at about lines 7, 23,
   54, 85, 115, 149, 180, 211, 242, and 273. Change nothing else.

**Tests:** (in `decision/config_tests.rs`. Each test parses TOML into
`LlmGlobalConfig` with `toml::from_str`.)
- `triage_config_parses`
  - setup: `[decision_providers.fake]\ntype = "mock"\n` followed by
    `test_support::TRIAGE_TOML`
  - assert: the set `triage` has 3 questions in order `is_billing, dept, urgency`.
    `to_questions()[1].kind` is `Choice` with values `[billing, tech]`.
    `questions[0].threshold == Some(0.8)`.
- `all_provider_types_parse`
  - setup: TOML with one entry each of `type = "openai"` (`api_key`,
    `default_model`), `"typesafe"` (same), `"ollama"` (`base_url`, `default_model`),
    and `"mock"` (no fields)
  - assert: `type_name()` returns the 4 names, and the mock's `predicate_probability
    == 1.0`
- `ollama_api_key_field_rejected_at_parse`
  - setup: an ollama entry with `api_key = "x"`
  - assert: `toml::from_str::<LlmGlobalConfig>` is `Err`
- `openai_debug_redacts_api_key`
  - setup: `api_key = "sk-secret-123"`
  - assert: `format!("{:?}", provider_config)` does not contain `sk-secret-123`
- `accessors_return_policy_fields`
  - setup: openai with `timeout_secs = 7`, `max_concurrency = 2`
  - assert: the accessors return `Some(7)` and `Some(2)`, and the mock returns `None`
- command: `cargo test -p camel-component-llm --lib decision::config`
- expected: fails before, passes after

**Acceptance:**
- All the tests above pass.
- `cargo test -p camel-component-llm --lib` passes (existing config tests unchanged
  apart from `..Default::default()`).

- [x] 1.4a

### Task 1.4b: Decision configuration validation

**Files:**
- `LLM/src/decision/config.rs` (modified)
- `LLM/src/decision/config_tests.rs` (modified)
- `LLM/src/config.rs` (modified)
- `LLM/src/lib.rs` (modified)

**Steps:**
1. Add `pub(crate) fn is_valid_question_name(s: &str) -> bool`. It returns true when
   the length is 1–64 and every char satisfies `c.is_ascii_alphanumeric() || c ==
   '_' || c == '-'`.
2. Add `pub(crate) fn validate_decision_config(global: &LlmGlobalConfig) ->
   Result<(), CamelError>`. Every error is `CamelError::Config`, with these messages:
   - The question count per set is 1–400. Message: `question set '{set}' must have
     1..=400 questions (got {n})`.
   - Name syntax: `question set '{set}': invalid question name '{name}'`.
   - Unique names: `question set '{set}': duplicate question name '{name}'`.
   - Blank `instructions`: `question set '{set}': question '{name}' has blank
     instructions`.
   - A choice question needs `choices` with 2–255 entries: `question set '{set}':
     choice '{name}' needs 2..=255 choices`. Values must be non-blank (`... blank
     choice value`) and unique (`... duplicate choice value '{v}'`).
   - A score question needs `levels` with 2–10 entries: `... score '{name}' needs
     2..=10 levels`. Labels must be non-blank.
   - `choices` on a non-choice question: `... '{name}': choices are only valid on
     choice questions`. `levels` on a non-score question: same pattern with `levels`
     and `score`.
   - `threshold` on a non-predicate: `... '{name}': threshold is only valid on
     predicate questions`. `min_confidence` on a predicate: `... '{name}':
     min_confidence is only valid on choice or score questions`.
   - `threshold` and `min_confidence` must be finite and in `[0,1]`: `... '{name}':
     threshold must be within [0, 1]` (the same wording for `min_confidence`).
   - Blank provider `api_key`, `base_url`, or `default_model`: `decision provider
     '{p}' {field} must not be blank`.
   - `timeout_secs == Some(0)` or `max_concurrency == Some(0)`: `decision provider
     '{p}' {field} must be > 0 when set (got 0)`.
   - Mock `predicate_probability` not finite or outside `[0,1]`: `decision provider
     '{p}' predicate_probability must be within [0, 1]`.
   - A mock `refuse` entry that fails `is_valid_question_name`: `decision provider
     '{p}': invalid refuse entry '{name}'`.
   - A `default_decision_provider` that is not a key of `decision_providers`:
     `default_decision_provider '{p}' is not configured`.
3. At the end of `LlmGlobalConfig::validate` in `config.rs`, call
   `crate::decision::config::validate_decision_config(self)?`.
4. In `lib.rs`, make `LlmComponent::new` call `config.validate()?` before it builds
   the provider map.

**Tests:** (in `decision/config_tests.rs`. Parse TOML, then call `validate()`. The
`valid` base is `[decision_providers.fake]\ntype = "mock"\n` + `TRIAGE_TOML`.)
- `valid_triage_config_passes`
  - assert: `validate()` is `Ok`
- `duplicate_question_name_rejected`
  - setup: `triage` with a second question named `dept`
  - assert: `Err`, and the text contains `triage` and `dept`
- `choice_with_one_option_rejected`
  - assert: `Err`, and the text contains `2..=255`
- `score_with_eleven_levels_rejected`
  - assert: `Err`, and the text contains `2..=10`
- `threshold_on_choice_rejected`
  - assert: `Err`, and the text contains `threshold`
- `min_confidence_on_predicate_rejected`
  - assert: `Err`, and the text contains `min_confidence`
- `levels_on_choice_rejected`
  - assert: `Err`, and the text contains `levels`
- `threshold_out_of_range_rejected`
  - setup: `threshold = 1.5`
  - assert: `Err`
- `invalid_question_name_rejected`
  - setup: name `"bad name"`
  - assert: `Err`, and the text contains `bad name`
- `empty_question_set_rejected`
  - setup: `[decision_question_sets.empty]\nquestions = []`
  - assert: `Err`, and the text contains `1..=400`
- `blank_openai_api_key_rejected`
  - setup: `api_key = "  "`
  - assert: `Err`, and the text contains `api_key`
- `zero_timeout_rejected`
  - setup: an openai entry with `timeout_secs = 0`
  - assert: `Err`
- `mock_probability_out_of_range_rejected`
  - setup: `predicate_probability = 2.0`
  - assert: `Err`
- `mock_refuse_invalid_name_rejected`
  - setup: `refuse = ["bad name"]`
  - assert: `Err`
- `unknown_default_decision_provider_rejected`
  - setup: `default_decision_provider = "ghost"`
  - assert: `Err`, and the text contains `ghost`
- `blank_instructions_rejected`
  - setup: `instructions = " "` on `dept`
  - assert: `Err`, and the text contains `blank instructions`
- `blank_choice_value_rejected`
  - setup: `{ value = "" }`
  - assert: `Err`, and the text contains `blank choice value`
- `duplicate_choice_value_rejected`
  - setup: `billing` twice
  - assert: `Err`, and the text contains `duplicate choice value`
- `blank_score_label_rejected`
  - setup: `{ label = "" }`
  - assert: `Err`
- `choices_on_predicate_rejected`
  - setup: `choices` on `is_billing`
  - assert: `Err`, and the text contains `choices`
- `four_hundred_one_questions_rejected`
  - setup: a set with 401 generated predicates
  - assert: `Err`, and the text contains `1..=400`
- `blank_base_url_rejected`
  - setup: ollama `base_url = " "`
  - assert: `Err`, and the text contains `base_url`
- `blank_default_model_rejected`
  - setup: openai `default_model = ""`
  - assert: `Err`, and the text contains `default_model`
- `zero_max_concurrency_rejected`
  - setup: `max_concurrency = 0`
  - assert: `Err`
- `component_new_runs_validate`
  - setup: the duplicate-name config
  - assert: `LlmComponent::new(cfg)` is `Err`
- command: `cargo test -p camel-component-llm --lib decision::config`
- expected: fails before, passes after

**Acceptance:**
- All the tests above pass.
- `cargo test -p camel-component-llm` passes, across all targets including `tests/`, because `LlmComponent::new` now validates.

- [x] 1.4b

### Task 1.5: Shared answer correlation and validation

**Files:**
- `LLM/src/decision/validate.rs` (new)
- `LLM/src/decision/mod.rs` (modified: add `#[cfg_attr(not(feature = "decisions"), allow(dead_code))] pub(crate) mod validate;`. The codecs in Tasks 2.3 and 2.4 are its only non-test callers. Task 2.5 replaced this with `#[cfg(any(feature = "decisions", test))]`.)

**Steps:**
1. Define these crate-internal codec output types:
   - `pub(crate) enum RawAnswer { Predicate { probability: f64 }, Choice { choice:
     String, confidence: Option<f64>, probabilities: Vec<ChoiceProbability> }, Score {
     score: f64, confidence: Option<f64>, probabilities: Vec<ScoreProbability> },
     Refusal }`
   - `pub(crate) struct RawNamedAnswer { pub(crate) name: String, pub(crate) answer:
     RawAnswer }`
2. Implement `pub(crate) fn correlate(questions: &[DecisionQuestion], raw:
   Vec<RawNamedAnswer>) -> Result<Vec<NamedAnswer>, LlmError>`. Every failure is
   `LlmError::Protocol(msg)`, where `msg` names the question:
   - Index `raw` by name. A repeated name → `duplicate answer for '{name}'`. A name
     that is not declared → `unknown answer '{name}'`.
   - For each question, in order: no answer → `missing answer for '{name}'`. A kind
     mismatch (for example, a `Choice` answer for a `Predicate` question) → `answer
     kind mismatch for '{name}'`. `Refusal` is allowed for any kind.
   - Predicate: check `probability` with `check_unit(name, "probability", p)`.
   - Choice:
     - `confidence` is `None` → `missing confidence for '{name}'`.
     - Check `confidence` and every probability with `check_unit`.
     - `choice` must be a declared value.
     - The distribution values must be unique and equal the declared set.
     - The sum must be within `0.01` of 1.
     - Output the probabilities reordered into declared order.
   - Score:
     - `confidence` is required, and checked as above.
     - `score` must be finite and in `[0 - 1e-9, levels-1 + 1e-9]`.
     - The distribution levels must be unique and equal `0..levels`.
     - The sum must be within `0.01` of 1.
     - Output the probabilities sorted by level.
3. `fn check_unit(name, field, v) -> Result<(), LlmError>`: `v.is_finite() && v >=
   -1e-9 && v <= 1.0 + 1e-9`. Values are never clamped.

**Tests:** (module `tests` in `validate.rs`. Use `test_support::triage_questions()`.)
- `correlates_out_of_order_answers`
  - action: raw answers in reverse order
  - assert: the output names are `[is_billing, dept, urgency]`
- `missing_answer_is_protocol`
  - action: omit `urgency`
  - assert: `Err(Protocol(m))`, and `m` contains `urgency`
- `duplicate_answer_is_protocol`
  - action: send `dept` twice
  - assert: `Protocol`
- `unknown_answer_is_protocol`
  - action: add an extra `foo`
  - assert: `Protocol`, and the text contains `foo`
- `kind_mismatch_is_protocol`
  - action: a `Predicate` answer for `dept`
  - assert: `Protocol`
- `refusal_accepted_for_any_kind`
  - action: `dept` is `Refusal`
  - assert: `Ok`, and `answers[1].answer == DecisionAnswer::Refusal`
- `probability_out_of_range_is_protocol`
  - action: predicate probability `1.5`
  - assert: `Protocol`
- `nan_probability_is_protocol`
  - action: `f64::NAN`
  - assert: `Protocol`
- `undeclared_choice_is_protocol`
  - action: `choice = "sales"`
  - assert: `Protocol`
- `choice_distribution_must_match_declared_set`
  - action: probabilities for `billing` only
  - assert: `Protocol`
- `choice_distribution_duplicate_value_is_protocol`
  - action: `billing` twice
  - assert: `Protocol`
- `distribution_sum_off_is_protocol`
  - action: probabilities `0.5, 0.3`
  - assert: `Protocol`
- `missing_confidence_is_protocol`
  - action: choice `confidence: None`
  - assert: `Protocol`, and the text contains `confidence`
- `score_out_of_range_is_protocol`
  - action: `score = 2.5` with 3 levels
  - assert: `Protocol`
- `fractional_score_preserved`
  - action: `score = 0.964` with probabilities levels `[2: 0.26, 0: 0.29, 1: 0.45]`
  - assert: `score == 0.964` and the probabilities `== [(0,0.29),(1,0.45),(2,0.26)]`
- `choice_probabilities_in_declared_order`
  - action: wire order `tech, billing`
  - assert: the output order is `billing, tech`
- `boundary_values_not_clamped`
  - action: probability `1.0 + 5e-10`
  - assert: `Ok`, and the value is stored unchanged
- command: `cargo test -p camel-component-llm --lib decision::validate`
- expected: fails before, passes after

**Acceptance:**
- All the tests above pass.
- Clippy is clean.

- [x] 1.5

### Task 1.6: Mock decision provider and decision provider map

**Files:**
- `LLM/src/decision/mock.rs` (new)
- `LLM/src/decision/factory.rs` (new)
- `LLM/src/decision/mod.rs` (modified: `pub mod mock; pub mod factory;`)

**Steps:**
1. `pub struct MockDecisionProvider { name: String, config: MockDecisionConfig }` with
   `pub fn new(name: impl Into<String>, config: MockDecisionConfig) -> Self`.
2. Implement `DecisionProvider`:
   - `id()` returns the name. `default_model()` returns `config.default_model`.
   - `capabilities()` returns `{ max_questions: 400 }`.
   - `decide` follows design.md "Mock" exactly:
     - `error_message` → `Err(LlmError::provider(msg))`.
     - A name in `refuse` → `Refusal`.
     - Predicate → `predicate_probability`.
     - Choice → the first value at 1.0, the others at 0.0, confidence 1.0.
     - Score → `score 0.0`, level 0 at 1.0, the others at 0.0, confidence 1.0.
     - `model` = `req.model`. `usage` = `DecisionUsage::default()`.
3. In `factory.rs`:
   - `pub type DecisionProviderMap = HashMap<String, Arc<dyn DecisionProvider>>;`
   - `pub fn build_decision_provider_map(config: &LlmGlobalConfig) ->
     Result<DecisionProviderMap, LlmError>`.
   - For `Mock`, build `MockDecisionProvider`.
   - For `Openai`, `Typesafe`, and `Ollama`, return
     `Err(LlmError::UnsupportedCapability(format!("decision provider '{name}' of type
     '{}' requires the 'decisions' feature", cfg.type_name())))`. Task 2.5 replaces
     these arms behind `#[cfg(feature = "decisions")]`.
   - Log construction errors with `tracing::error!` (with a `// log-policy:
     system-broken` comment), as `build_provider_map` does.

**Tests:** (module `tests` in `mock.rs` and in `factory.rs`)
- `mock_answers_mixed_set`
  - setup: `predicate_probability = 0.25`, `refuse = ["urgency"]`, the triage
    questions
  - action: `decide`
  - assert: `Predicate{0.25}`; `Choice{choice:"billing", confidence:1.0,
    probabilities:[billing 1.0, tech 0.0]}`; `Refusal`
- `mock_score_defaults_to_level_zero`
  - assert: `Score{score:0.0, confidence:1.0, probabilities:[(0,1.0),(1,0.0),(2,0.0)]}`
- `mock_error_message_fails`
  - setup: `error_message = Some("boom")`
  - assert: `Err(Provider(m))`, and `m` contains `boom`
- `mock_reports_400_max_questions`
- `factory_builds_mock`
  - assert: the map contains the key, and `id() == key`
- `factory_rejects_openai_without_feature` (`#[cfg(not(feature = "decisions"))]`)
  - assert: `Err(UnsupportedCapability(m))`, and `m` contains `decisions`
- command: `cargo test -p camel-component-llm --lib decision::`
- expected: fails before, passes after

**Acceptance:**
- All the tests above pass.
- Clippy is clean.

- [x] 1.6

### Task 1.7: Decide operation in the endpoint URI and metadata

**Files:**
- `LLM/src/config.rs` (modified)
- `LLM/src/config_tests.rs` (modified)
- `LLM/src/metadata.rs` (modified)

**Steps:**
1. Add `Decide` to `LlmOperation` and add `#[non_exhaustive]`. Add only an explicit `Decide` arm to
   each in-crate `match` on `LlmOperation` (the one in `producer.rs` `call`). Do not
   add a wildcard arm in-crate. The `LlmProducer::call` arm for `Decide`
   returns `Err(LlmError::UnsupportedCapability("decide is handled by
   DecisionProducer".into()))`.
2. Add `pub questions: Option<String>` to `LlmEndpointConfig` and its `Default`.
3. In `from_uri`:
   - Parse `"decide"` → `LlmOperation::Decide`.
   - Update the unknown-op message to `expected 'chat', 'embed' or 'decide'`.
   - Detect presence with `params.contains_key(..)`, because the parsed `stream` defaults
     to `true`.
   - For `Decide`, reject the presence of `stream`, `temperature`, `max_tokens`, or
     `system_prompt` with `InvalidUri("parameter '<p>' is not supported by
     llm:decide")`, and require `questions` (`InvalidUri("llm:decide requires
     'questions'")`).
   - For `Chat` and `Embed`, reject `questions` with `InvalidUri("parameter
     'questions' is only supported by llm:decide")`.
   - Set `questions` from the params.
4. In `metadata.rs`, add `#[uri_param(name = "questions")] pub _questions:
   Option<String>`. Change the description to `"LLM chat / embed / decide
   producer"`. Add `"questions"` to the parity test's expected set.

**Tests:** (in `config_tests.rs`)
- `decide_uri_parses`
  - action: `from_uri("llm:decide?questions=triage&provider=fake&model=m")`
  - assert: `operation == Decide`, `questions == Some("triage")`,
    `provider == Some("fake")`, `model == Some("m")`
- `decide_requires_questions`
  - action: `from_uri("llm:decide")`
  - assert: `Err(InvalidUri(m))`, and `m` contains `questions`
- `decide_rejects_temperature`
  - action: `from_uri("llm:decide?questions=t&temperature=0.2")`
  - assert: `Err`, and the text contains `temperature`
- `decide_rejects_stream`
  - action: `from_uri("llm:decide?questions=t&stream=false")`
  - assert: `Err`, and the text contains `stream`
- `chat_rejects_questions`
  - action: `from_uri("llm:chat?questions=t")`
  - assert: `Err`, and the text contains `questions`
- `llm_metadata_uri_options_parity` (updated)
  - assert: it includes `questions`
- command: `cargo test -p camel-component-llm --lib`. This covers `config` and
  `metadata::tests`.
- expected: new tests fail before, pass after

**Acceptance:**
- `cargo test -p camel-component-llm --lib` passes.

- [x] 1.7

### Task 1.8: Decision producer

**Files:**
- `LLM/src/decision/producer.rs` (new)
- `LLM/src/decision/producer_tests.rs` (new; `#[cfg(test)] #[path = "producer_tests.rs"] mod tests;`)
- `LLM/src/decision/mod.rs` (modified: `pub mod producer;`)

**Steps:**
1. Define this struct and derive `Clone`:
   `pub struct DecisionProducer { provider: Arc<dyn DecisionProvider>, model: String,
   set: Arc<QuestionSetConfig>, questions: Arc<[DecisionQuestion]>, max_state_bytes:
   usize, semaphore: Option<Arc<Semaphore>>, timeout: Option<Duration>, retry:
   Option<NetworkRetryPolicy>, route_id: String }`
2. Add the constructor and builder methods:
   - `pub fn new(provider, model_override: Option<String>, set:
     Arc<QuestionSetConfig>, max_state_bytes: usize, route_id: String) -> Self`.
     `model` = the override, or `provider.default_model()`. `questions` =
     `set.to_questions()`.
   - Builder methods `with_semaphore`, `with_timeout`, and `with_retry`, each
     returning `Self`.
3. `fn extract_state(&self, body: &Body) -> Result<DecisionState, LlmError>` maps the
   body as follows:
   - `Text`, `Xml`, UTF-8 `Bytes`, and `Json(String)` → `Text`.
   - A `Json` object or array → `Json`.
   - A `Json` number, bool, or null; `Empty`; `Stream`; and invalid UTF-8 →
     `InvalidRequest`.

   `Body` is a foreign `#[non_exhaustive]` enum, so add a `_ =>` arm that returns
   `InvalidRequest("unsupported body type for llm:decide")`. The in-crate no-wildcard
   rule applies only to `LlmOperation`. The details:
   - `Bytes` uses `String::from_utf8`. An error → `InvalidRequest("decision state is
     not valid UTF-8")`.
   - The size check uses `serde_json::to_vec(&v).len()` for JSON and `s.len()` for
     text. Over `max_state_bytes` → `InvalidRequest("decision state exceeds {max}
     bytes")`.
4. `async fn handle(&self, exchange: &mut Exchange) -> Result<(), LlmError>`:
   1. Extract the state.
   2. Build `DecisionRequest`.
   3. Run `crate::producer::apply_timeout(self.timeout,
      crate::producer::run_with_retry(self.semaphore.clone(), self.retry.clone(),
      Arc::new(AtomicBool::new(false)), &self.route_id, op))`, where `op` clones the
      request and calls `provider.decide`.
   4. Write the body with `fn answers_json(&self, resp: &DecisionResponse) -> Value`,
      following the design.md "Endpoint and producer" body shape:
      - Choice probabilities form a JSON object `{value: p}`. Key order is not
        semantic.
      - Score probabilities form an array of `{"level","label","probability"}`. The
        label comes from the question's `levels`.
      - `meets_threshold` is added only if the question's `threshold` is `Some`, and
        `meets_min_confidence` only if `min_confidence` is `Some`.
      - Refusal is `{"type":"refusal"}` only.
   5. Set the headers `CAMEL_LLM_PROVIDER = provider.id()`, `CAMEL_LLM_MODEL =
      resp.model`, and `CAMEL_LLM_USAGE_AVAILABLE = true`. Set
      `CAMEL_LLM_TOKENS_IN`, `CAMEL_LLM_TOKENS_OUT`, `CAMEL_LLM_CACHED_TOKENS_IN`,
      and `CAMEL_LLM_CACHE_WRITE_TOKENS_IN` each only when `Some`.
5. Implement `tower::Service<Exchange>` like `LlmProducer`:
   - `poll_ready` returns `Ready(Ok)`.
   - `call` clones `self` and runs `handle`.
   - On an error: `tracing::warn!(route_id, error, "llm decide producer error")`,
     then `Err(CamelError::from(e))`.

**Tests:** (`decision/producer_tests.rs`. Every test drives
`tower::Service::call(&mut producer, exchange)`, so the errors are `CamelError`.
Build the Exchange with `Exchange::new(Message::new(body))`.
- Set helper: add `pub(crate) fn triage_set() -> QuestionSetConfig` to
  `decision::test_support`. It parses `TRIAGE_TOML` into `LlmGlobalConfig` with
  `toml::from_str` and returns `decision_question_sets["triage"].clone()`.
- Mock: `MockDecisionProvider::new("fake", MockDecisionConfig { default_model:
  "mock-decision".into(), predicate_probability: 1.0, refuse: vec![], error_message:
  None })`. Change single fields per test.
- `RecordingProvider` (test-local): `id` "rec", `default_model` "rec-model",
  `max_questions` 400. It stores the last `DecisionRequest` in a
  `Mutex<Option<DecisionRequest>>` and returns `DecisionResponse { model:
  "rec-model".into(), answers: <Predicate{0.9}, Choice{"billing", 0.9,
  [billing 0.9, tech 0.1]}, Score{1.0, 0.8, [(0,0.1),(1,0.8),(2,0.1)]}> named
  is_billing/dept/urgency, usage: <per test, default all None> }`.
- Producer: `DecisionProducer::new(provider, None, Arc::new(triage_set()), 32768,
  "test-route".into())`.)
- `json_number_body_rejected`
  - action: `Body::Json(json!(42))`
  - assert: a `CamelError` whose text contains `invalid request`
- `empty_body_rejected`
  - assert: as above
- `json_string_body_is_text_state`
  - action: `Body::Json(json!("hello"))`
  - assert: the recorded state `== DecisionState::Text("hello")`
- `json_object_body_is_json_state`
  - assert: the recorded state is `Json`
- `invalid_utf8_bytes_rejected`
  - action: `Body::Bytes(bytes::Bytes::from(vec![0xff, 0xfe]))`
  - assert: `Err`
- `oversize_state_rejected`
  - setup: `max_state_bytes = 4`
  - action: text `"hello"`
  - assert: `Err`, and the text contains `exceeds`
- `output_body_shape`
  - setup: mock (probability 1.0), and the canonical triage set parsed from
    `test_support::TRIAGE_TOML` (`threshold = 0.8` on `is_billing`, no
    `min_confidence`)
  - assert: `body.answers.is_billing == {"type":"predicate","probability":1.0,
    "meets_threshold":true}`, `body.answers.dept.probabilities ==
    {"billing":1.0,"tech":0.0}`, `body.answers.dept` has no `meets_min_confidence`
    key, and `body.answers.urgency.probabilities[0] ==
    {"level":0,"label":"low","probability":1.0}`
- `no_threshold_no_meets_key`
  - setup: the triage set with `threshold` removed from `is_billing` (edit the parsed
    `QuestionSetConfig` in the test)
  - assert: the predicate object has no `meets_threshold` key
- `refusal_has_no_meets_keys`
  - setup: `refuse = ["is_billing"]`, `threshold = 0.8`
  - assert: `answers.is_billing == {"type":"refusal"}`
- `usage_headers_only_when_reported`
  - setup: `RecordingProvider` usage `DecisionUsage { input_tokens: Some(375), output_tokens: Some(0),
    cached_input_tokens: None, cache_write_input_tokens: None }`
  - assert: `CamelLlmTokensIn == 375`, `CamelLlmTokensOut == 0`, and
    `CamelLlmCachedTokensIn` is absent
- `openai_style_cached_usage_headers`
  - setup: usage `DecisionUsage { input_tokens: Some(2000), output_tokens: Some(0),
    cached_input_tokens: Some(1500), cache_write_input_tokens: Some(0) }`
  - assert: all four headers are set with those values
- `no_cost_header`
  - assert: `CamelLlmEstimatedCostUsd` is absent
- `provider_and_model_headers`
  - assert: `CamelLlmProvider == "fake"`, and `CamelLlmModel` equals the mock
    default model
- command: `cargo test -p camel-component-llm --lib decision::producer`
- expected: fails before, passes after

**Acceptance:**
- All the tests above pass.
- Clippy is clean.

- [x] 1.8

### Task 1.9a: Component, endpoint, and bundle wiring

**Files:**
- `LLM/src/decision/endpoint.rs` (new)
- `LLM/src/decision/mod.rs` (modified: `pub mod endpoint;`)
- `LLM/src/lib.rs` (modified)
- `LLM/src/bundle.rs` (modified)

**Steps:**
1. In `decision/endpoint.rs`, define this struct:
   `pub struct DecisionEndpoint { uri: String, config: LlmEndpointConfig,
   provider_name: String, provider: Arc<dyn DecisionProvider>, set:
   Arc<QuestionSetConfig>, global: Arc<LlmGlobalConfig> }`, with `pub(crate) fn
   new(...)` taking all six fields. Implement `Endpoint` for it:
   - `uri()` returns `uri`.
   - `create_producer(rt, ctx)`:
     - Look up `global.decision_providers.get(&provider_name)` and read
       `max_concurrency()`, `timeout_secs()`, and `network_retry()` from it.
     - The effective timeout is `provider_timeout.or(global.timeout_secs).map(Duration::from_secs)`.
     - `route_id` = `ctx.route_id().unwrap_or("unknown")`.
     - Build `DecisionProducer::new(Arc::clone(&provider), config.model.clone(),
       Arc::clone(&set), global.max_prompt_bytes, route_id)`, then
       `.with_semaphore(max_concurrency.map(|n| Arc::new(Semaphore::new(n))))`,
       `.with_timeout(timeout)`, and `.with_retry(network_retry)`.
     - Return `Ok(BoxProcessor::new(producer))`. `rt` is unused, so name it `_rt`.
   - `create_consumer` returns `Err(CamelError::InvalidUri("llm component does not
     support consumers in MVP".into()))`.
2. In `lib.rs`, `LlmComponent`:
   - Add the field `decision_providers: Arc<DecisionProviderMap>`.
   - `new()` runs `validate()` (already added in 1.4b), `build_provider_map`, and
     `crate::decision::factory::build_decision_provider_map(&config)`, mapping errors
     with `CamelError::from`.
   - `from_parts` sets `Arc::new(HashMap::new())`. Add the doc line "Decision
     providers start empty; chain `with_decision_providers`."
   - Add `pub fn with_decision_providers(mut self, map: DecisionProviderMap) -> Self`
     and `pub fn decision_providers(&self) -> &DecisionProviderMap`.
3. In `lib.rs`, `create_endpoint`: right after `LlmEndpointConfig::from_uri(uri)?`,
   add `if endpoint_config.operation == LlmOperation::Decide { return
   self.create_decide_endpoint(uri, endpoint_config); }`, before any chat-provider
   lookup. Add the private method `fn create_decide_endpoint(&self, uri: &str,
   endpoint_config: LlmEndpointConfig) -> Result<Box<dyn Endpoint>, CamelError>`:
   - Resolve the name from `endpoint_config.provider` or
     `self.config.default_decision_provider`. If neither is set →
     `InvalidUri("no decision provider specified and no default_decision_provider
     configured")`.
   - Look up `self.decision_providers`. Missing → `InvalidUri("decision provider '{n}'
     not found in config")`.
   - The set name is `endpoint_config.questions` (always `Some` for decide). Look up
     `self.config.decision_question_sets`. Missing → `InvalidUri("question set '{q}'
     not found in config")`.
   - If `set.questions.len() > provider.capabilities().max_questions` →
     `InvalidUri("question set '{q}' has {len} questions; decision provider '{n}'
     accepts at most {max}")`.
   - Return `Box::new(DecisionEndpoint::new(uri.to_string(), endpoint_config, name,
     provider, Arc::new(set.clone()), Arc::clone(&self.config)))`.
4. In `bundle.rs`:
   - Add the field `decision_providers: DecisionProviderMap` to `LlmBundle`.
   - In `from_toml`, after `validate()`, build it with `build_decision_provider_map`
     (`.map_err(CamelError::from)`).
   - In `register_all`, use
     `LlmComponent::from_parts(self.config, self.providers).with_decision_providers(self.decision_providers)`.

**Tests:** (module `decide_tests` in `decision/endpoint.rs`. Build `LlmComponent::new`
from TOML: `[decision_providers.fake]\ntype = "mock"\n` + `TRIAGE_TOML`. Where stated, **prepend** `default_decision_provider = "fake"\n` before every table header; if it is appended, it lands inside a table.)
- `decide_endpoint_unknown_set`
  - action: `create_endpoint("llm:decide?provider=fake&questions=nope",
    &NoOpComponentContext)`
  - assert: `Err`, and the text contains `nope`
- `decide_endpoint_unknown_set_via_default_provider`
  - setup: prepend `default_decision_provider = "fake"`
  - action: `create_endpoint("llm:decide?questions=nope", &NoOpComponentContext)`
  - assert: `Err`, and the text contains `nope`
- `decide_endpoint_unknown_provider`
  - action: `llm:decide?provider=ghost&questions=triage`
  - assert: `Err`, and the text contains `ghost`
- `decide_endpoint_no_provider`
  - setup: no `default_decision_provider`
  - action: `llm:decide?questions=triage`
  - assert: `Err`, and the text contains `default_decision_provider`
- `decide_with_zero_chat_providers_succeeds`
  - setup: no `[providers]`, and `default_decision_provider = "fake"`
  - assert: `create_endpoint("llm:decide?questions=triage")` is `Ok`
- `decide_endpoint_creates_producer`
  - action: on the endpoint above, `create_producer(Arc::new(NoopRuntimeObservability),
    &ProducerContext::new().with_route_id("r"))`
  - assert: `Ok`
- `from_parts_has_empty_decision_map`
  - assert: `LlmComponent::from_parts(LlmGlobalConfig::default(),
    HashMap::new()).decision_providers().is_empty()`
- `bundle_wires_decision_providers` (in `bundle.rs` tests)
  - setup: `LlmBundle::from_toml` with the mock decision provider and triage
  - action: `register_all` into a new test struct `ComponentCapture` (in the `bundle.rs`
    test module) that implements `ComponentRegistrar` and stores the
    `Arc<dyn Component>`. The existing `TestRegistrar` stores only scheme strings.
  - assert: `component.create_endpoint("llm:decide?provider=fake&questions=triage",
    &NoOpComponentContext)` is `Ok`
- command: `cargo test -p camel-component-llm --lib`
- expected: new tests fail before, pass after

**Acceptance:**
- All the tests above pass.
- `cargo test -p camel-component-llm` passes (default features).

- [x] 1.9a

### Task 1.9b: Route tests for llm:decide

**Files:**
- `LLM/tests/common/decision.rs` (new)
- `LLM/tests/common/mod.rs` (modified: add `pub mod decision;`, and add `#[allow(dead_code)]` to `make_producer_ctx`, because `decision_route.rs` does not call it)
- `LLM/tests/decision_route.rs` (new)

**Steps:**
1. In `tests/common/decision.rs`, define `pub const TRIAGE_TOML`,
   `pub const OPENAI_TRIAGE_RESPONSE`, and `pub const SYSTEM_ONE_TRIAGE_RESPONSE`,
   exactly as in "Shared test fixtures". Add `#![allow(dead_code)]`.
2. In `tests/decision_route.rs`, add a helper `fn register(ctx: &mut CamelContext)`.
   It builds `LlmBundle::from_toml` from this TOML (no chat `[providers]` table),
   then calls `bundle.register_all(ctx)`:
   ```
   default_decision_provider = "fake"
   [decision_providers.fake]
   type = "mock"
   predicate_probability = 1.0
   ```
   followed by `common::decision::TRIAGE_TOML`.
3. The spec scenarios say `direct:in`. A one-shot timer plus `process` is the
   equivalent harness used in this crate, because it needs no producer template.
   Every route starts from `timer:<unique-name>?period=10&repeatCount=1`, followed by
   `.process(|mut ex| async move { ex.input.body = Body::Text("My invoice was charged
   twice".into()); Ok(ex) })`. This is the pattern in `tests/dsl_route.rs:36-49`.
   Register `TimerComponent::new()` and a `MockComponent`. After `ctx.start()`, wait
   only with `endpoint.await_exchanges(1, Duration::from_secs(5)).await`, then call
   `ctx.stop()`. Use no `sleep`.

**Tests:**
- `decide_simple_threshold_route`
  - setup: the route `timer → process → .to("llm:decide?provider=fake&questions=triage")
    → .validate("${body.answers.is_billing.meets_threshold} == true") →
    .to("mock:billing")`
  - assert: `mock:billing` has exactly 1 Exchange
- `decide_choice_route`
  - setup: `.to("llm:decide?questions=triage")`, using the default provider, then:
    ```rust
    .choice()
        .when(|ex: &Exchange| match &ex.input.body {
            Body::Json(v) => v.pointer("/answers/dept/choice") == Some(&serde_json::json!("billing")),
            _ => false,
        })
        .to("mock:billing")
        .end_when()
        .otherwise()
        .to("mock:other")
        .end_otherwise()
    .end_choice()
    ```
    These are the builder methods (`camel-builder/src/lib.rs:1551-1618`): `when`, `end_when`, `otherwise`, `end_otherwise`, and `end_choice`.
  - assert: `mock:billing` has 1 Exchange, and `mock:other` has 0
- `decide_route_sets_headers`
  - setup: the same route as `decide_simple_threshold_route`, with its own timer name
  - action: wait for `mock:billing`, then read its received Exchange
  - assert: the received Exchange has `CamelLlmProvider == "fake"` and
    `CamelLlmUsageAvailable == true`
- command: `cargo test -p camel-component-llm --test decision_route`
- expected: fails before, passes after

**Acceptance:**
- All the tests above pass.
- `cargo xtask lint-test-sleep` and `cargo xtask lint-unbounded-wait` exit 0.

- [x] 1.9b

## Phase 2: HTTP adapters and docs

### Task 2.1: Move the shared HTTP status mapping out of the genai adapter

**Files:**
- `LLM/src/provider/http_errors.rs` (new)
- `LLM/src/provider/mod.rs` (modified)
- `LLM/src/provider/genai_adapter.rs` (modified)
- `LLM/src/provider/genai_adapter_tests.rs` (modified only if paths break)

**Steps:**
1. Move these functions byte-for-byte from `genai_adapter.rs` into
   `provider/http_errors.rs` (no genai import), and make each one `pub(crate)`:
   `truncate_body` (with `MAX_BODY_ERROR_BYTES`), `map_reqwest_error`, `map_status`,
   `is_insufficient_quota`, and `parse_retry_after`.
2. In `provider/mod.rs`, add `#[cfg(any(feature = "decisions", feature = "openai",
   feature = "ollama", feature = "generic-providers", feature = "all-providers"))]
   pub(crate) mod http_errors;`. The `decisions` feature was declared in Task 1.3.
3. In `genai_adapter.rs`, import only the three functions that it calls directly:
   `map_status`, `map_reqwest_error`, and `parse_retry_after`, from
   `crate::provider::http_errors`. `truncate_body` and `is_insufficient_quota` are
   called only inside `map_status`. Importing them gives `unused_imports`.
4. In `genai_adapter_tests.rs`, add `use crate::provider::http_errors::is_insufficient_quota;`
   (the test at about line 537 calls it). Add any other moved function that
   `super::*` no longer reaches.

**Tests:**
- No new tests. The existing `genai_adapter_tests` status-mapping tests are the
  regression net.
  - command: `cargo test -p camel-component-llm --lib --features all-providers`
  - expected: pass before and after
- `cargo test -p camel-component-llm --test boundary --features all-providers` passes.

**Acceptance:**
- Both commands pass.
- `grep -n "fn map_status" LLM/src/provider/genai_adapter.rs` returns nothing.

- [x] 2.1

### Task 2.2: Shared decision HTTP transport

**Files:**
- `LLM/src/decision/http.rs` (new; `#[cfg(feature = "decisions")]`)
- `LLM/src/decision/mod.rs` (modified)

**Steps:**
1. `pub(crate) const MAX_DECISION_RESPONSE_BYTES: usize = 8 * 1024 * 1024;`
2. `pub(crate) fn join_url(base: &str, path: &str) -> String`, implemented as
   `format!("{}{}", base.trim_end_matches('/'), path)`.
3. Define `pub(crate) struct DecisionHttp { url: String, base_url: String,
   explicit_base_url: bool, api_key: Option<String>, policy: SsrfPolicy, timeout:
   Duration, client: tokio::sync::OnceCell<reqwest::Client> }`. Write a manual
   `Debug` that prints `api_key` as `"[REDACTED]"` and the URL through
   `camel_api::redact::redact_url`.
4. `async fn client(&self) -> Result<&reqwest::Client, LlmError>` uses
   `OnceCell::get_or_try_init`:
   - If `explicit_base_url`, run `crate::provider_factory::validate_llm_url_pinned(
     &base_url, policy)` in `tokio::task::spawn_blocking`. A join error →
     `InvalidRequest`. Then pin `Some((host, &addrs))` only when the base URL host is
     a `url::Host::Domain`.
   - Otherwise, use no pinning.
   - Build with `crate::hardened_http_client_with_pinning(pinning)`. A `CamelError`
     → `LlmError::provider(e.to_string())`.
5. `pub(crate) async fn post_json(&self, body: Vec<u8>, model: &str) ->
   Result<Bytes, LlmError>`:
   1. `client.post(&self.url).header(CONTENT_TYPE, "application/json")`, plus
      `.bearer_auth(key)` if there is a key, `.timeout(self.timeout)`, and
      `.body(body)`.
   2. A send error → `http_errors::map_reqwest_error(&e, self.timeout)`.
   3. Read the body with a `while let Some(chunk) = resp.chunk().await` loop.
      `chunk()` errors map through `map_reqwest_error`. Past
      `MAX_DECISION_RESPONSE_BYTES` → `Protocol("decision response exceeds 8 MiB")`.
   4. On a 2xx status, return the bytes.
   5. Otherwise, take the text with `String::from_utf8_lossy`. 413 or 415 →
      `InvalidRequest(http_errors::truncate_body(&text))`. For any other status,
      call `http_errors::map_status(resp.status().as_u16(), &text, Some(model),
      http_errors::parse_retry_after(retry_after_header))` with the WHOLE capped
      text. Do not pre-truncate it: the `insufficient_quota` check parses the
      body as JSON, and `map_status` truncates every error text itself (amended
      2026-11-03).
6. Add `pub(crate) fn new(base_url: String, explicit_base_url: bool, path: &str,
   api_key: Option<String>, policy: SsrfPolicy, timeout: Duration) -> Self`. It sets
   `url = join_url(&base_url, path)`.

**Tests:** (module `tests` in `http.rs`, `#[cfg(feature = "decisions")]`,
wiremock)
- `join_url_trims_trailing_slash`
  - assert: `join_url("https://a/v1/", "/decisions") == "https://a/v1/decisions"`
- `post_json_returns_body_on_200`
  - setup: wiremock POST `/x` → 200 `{"ok":1}`. `DecisionHttp::new(server.uri(), true,
    "/x", Some("k".into()), SsrfPolicy::AllowInternal, Duration::from_secs(5))`
  - assert: the bytes equal the body, and the request had `authorization: Bearer k`
- `no_auth_header_without_key`
  - setup: wiremock POST `/x` → 200 `{}`. `DecisionHttp::new(server.uri(), true,
    "/x", None, SsrfPolicy::AllowInternal, Duration::from_secs(5))`.
  - action: `post_json(b"{}".to_vec(), "m")`
  - assert: the received request has no `authorization` header
- `oversize_response_is_protocol`
  - setup: a 200 body of `8 MiB + 1` bytes
  - assert: `Err(Protocol(_))`
- `debug_redacts_api_key`
  - setup: `api_key = Some("sk-secret-123")`
  - assert: `format!("{:?}")` does not contain it
- `status_413_and_415_are_invalid_request`
  - setup: for each status in `[413, 415]`, a fresh wiremock POST `/x` returning that
    status with body `"too big"`, and `DecisionHttp::new(server.uri(), true, "/x",
    Some("k".into()), SsrfPolicy::AllowInternal, Duration::from_secs(5))`
  - assert: `post_json` gives `Err(LlmError::InvalidRequest(_))` for both
- command: `cargo test -p camel-component-llm --lib --features decisions decision::http`
- expected: fails before, passes after

**Acceptance:**
- All the tests above pass.
- Clippy is clean with the default features. With `--features decisions`, run
  `cargo clippy -p camel-component-llm --all-targets --features decisions -- -D warnings -A dead_code`.
  It must exit 0. `DecisionHttp` has no non-test caller until Task 2.3, and the factory
  arrives in Task 2.5.

- [x] 2.2

### Task 2.3: OpenAI Decisions provider and codec

**Files:**
- `LLM/src/decision/openai.rs` (new; `#[cfg(feature = "decisions")]`)
- `LLM/src/decision/mod.rs` (modified: `#[cfg(feature = "decisions")] pub(crate) mod openai;`)

**Steps:**
0. Define `pub(crate) struct OpenaiDecisionProvider { name: String, default_model: String, http: DecisionHttp }`. Its `pub(crate) fn new(name: &str, cfg: &OpenaiDecisionConfig, policy: SsrfPolicy, global_timeout: Option<u64>) -> Self` sets base = `cfg.base_url` or `"https://api.openai.com/v1"`, `explicit_base_url = cfg.base_url.is_some()`, path `"/decisions"`, key `Some(cfg.api_key)`, and timeout = `Duration::from_secs(cfg.timeout_secs.or(global_timeout).unwrap_or(30))`. Derive or implement `Debug` (the `DecisionHttp` Debug already redacts). Its `DecisionProvider` impl: `id` = name, `default_model`, and `capabilities` = `{ max_questions: 400 }`.
1. Write private wire structs (`serde`, no `deny_unknown_fields` on responses) that
   match the design.md OpenAI request and response examples. Answers use a
   `#[serde(tag = "type", rename_all = "lowercase")]` enum with `Predicate {
   name: Option<String>, probability: f64 }`, `Choice { name, choice: String,
   confidence: Option<f64>, probabilities: Vec<{value: String, probability: f64}> }`,
   `Score { name, score: f64, confidence: Option<f64>, probabilities: Vec<{label:
   String, value: i64, probability: f64}> }`, and `Refusal { name }`. Usage structs
   use `Option` fields, including `input_tokens_details.{cached_tokens,
   cache_write_tokens}`.
2. Implement `pub(crate) fn build_request(req: &DecisionRequest) -> Result<Vec<u8>,
   LlmError>`:
   - `Text(s)` → `"input": s`.
   - A `Json` object, or an array whose elements are all objects →
     `"input":[{"role":"user","content":[{"type":"input_json","data":<value>}]}]`.
   - A `Json` array with a non-object element → `InvalidRequest("openai decisions
     require a JSON object or an array of objects")`.
   - Questions: `{"type":"predicate"|"choice"|"score","name","instructions"}`, plus
     `"choices":[{"value","description"?}]` or `"levels":[{"label","description"?}]`.
     Omit a `description` that is `None`.
3. Implement `pub(crate) fn parse_response(questions: &[DecisionQuestion], bytes:
   &[u8]) -> Result<DecisionResponse, LlmError>`:
   - A decode error → `Protocol("malformed openai decision response: {e}")`, with
     the message truncated through `LlmError::protocol`-style truncation at 200
     bytes. Use `crate::provider::http_errors::truncate_body` on `e.to_string()`.
   - An answer with no `name` → `Protocol("openai answer without name")`.
   - Map to `RawNamedAnswer`. A score `value < 0` → `Protocol`. Map it to `level:
     value as usize` and ignore the wire `label`.
   - Call `validate::correlate`.
   - `DecisionResponse.model` = the wire `model` (a required `String`; missing →
     `Protocol`).
   - Usage: `input_tokens`, `output_tokens`, `cached_tokens`, `cache_write_tokens` →
     `DecisionUsage`.
4. `decide` runs `build_request`, then `self.http.post_json(body, &req.model).await`, then `parse_response`.

**Tests:** (module `tests` in `openai.rs`, pure functions, no network)
- `request_text_state_and_all_kinds`
  - setup: triage questions, `Text("hi")`
  - assert: the parsed JSON has `input == "hi"`. `questions[1]` is
    `{"type":"choice","name":"dept","instructions":"Which department?",
    "choices":[{"value":"billing","description":"payments"},{"value":"tech"}]}`.
    `questions[2].levels[0] == {"label":"low"}`.
- `request_json_object_state`
  - assert: `input[0].content[0] == {"type":"input_json","data":{"a":1}}`
- `request_json_array_of_scalars_rejected`
  - assert: `Err(InvalidRequest(_))`
- `parse_design_example`
  - setup: `OPENAI_TRIAGE_RESPONSE` with `{"type":"refusal","name":"other"}`
    appended to `answers`, and the triage questions plus a fourth predicate `other`
  - assert: the 4 answers are typed, `dept.choice == "billing"`, `urgency.score ==
    0.96`, `other == Refusal`, and the usage is `{2000, 0, 1500, 0}`
- `parse_reverse_order`
  - assert: the result is in question order
- `parse_duplicate_name_is_protocol`
  - setup: `OPENAI_TRIAGE_RESPONSE` with
    `{"type":"predicate","name":"is_billing","probability":0.5}` appended to
    `answers`
  - assert: `Err(Protocol(_))`
- `parse_duplicate_probability_value_is_protocol`
  - setup: a second `{"value":"billing","probability":0.0}` added to the `dept`
    probabilities
  - assert: `Err(Protocol(_))`
- `parse_missing_confidence_is_protocol`
  - setup: `"confidence":0.99` removed from `dept`
  - assert: `Err(Protocol(m))`, and `m` contains `confidence`
- `provider_debug_redacts_key`
  - setup: `OpenaiDecisionProvider::new("oai", &cfg)` with `api_key = "sk-secret-123"`,
    `SsrfPolicy::AllowInternal`, `None`
  - assert: `format!("{:?}", provider)` does not contain `sk-secret-123`
- `parse_ignores_wire_label`
  - setup: a score wire label `"LOW"`
  - assert: `Ok`
- `parse_malformed_json_is_protocol`
  - setup: `b"not json"`
- command: `cargo test -p camel-component-llm --lib --features decisions decision::openai`
- expected: fails before, passes after

**Acceptance:**
- The tests above pass.

- Until Task 2.5 wires the factory, the new provider struct has no non-test caller. For this task only, run clippy as `cargo clippy -p camel-component-llm --all-targets --features decisions -- -D warnings -A dead_code`. That must exit 0. Task 2.5 restores the full gate.

- [x] 2.3

### Task 2.4: System One provider and codec (TypeSafe and Ollama)

**Files:**
- `LLM/src/decision/system_one.rs` (new; `#[cfg(feature = "decisions")]`)
- `LLM/src/decision/mod.rs` (modified: `#[cfg(feature = "decisions")] pub(crate) mod system_one;`)

**Steps:**
0. Define `pub(crate) struct SystemOneDecisionProvider { name: String, default_model: String, http: DecisionHttp, max_questions: usize }` with two constructors:
   - `new_typesafe(name: &str, cfg: &TypesafeDecisionConfig, policy, global_timeout)`: base = `cfg.base_url` or `"https://api.typesafe.ai"`, `explicit_base_url = cfg.base_url.is_some()`, key `Some(cfg.api_key)`, `max_questions = 400`.
   - `new_ollama(name: &str, cfg: &OllamaDecisionConfig, policy, global_timeout)`: base = `cfg.base_url`, `explicit_base_url = true`, key `None`, `max_questions = 64`.
   - For both: path `"/v1/systemone"` and the same timeout rule as OpenAI. `capabilities` returns `max_questions`.
1. Implement `pub(crate) fn build_request(req: &DecisionRequest) -> Result<Vec<u8>,
   LlmError>`:
   - `{"model", "state", "questions": {name: q}}`, with the keys in question order.
     Use a `serde_json::Map`; the `preserve_order` feature is not required, because
     the order of a JSON object is not semantic.
   - `Text(s)` → `"state": s`. A `Json` object or array → `"state": value`.
   - Predicate → `{"type":"noul","instructions"}`.
   - Choice → `{"type":"choice","instructions","criteria":{value: description or
     null}}`.
   - Score → `{"type":"score","instructions","criteria":[label or "label:
     description"]}`.
2. Decode the response into private structs:
   - `answers: DupCheckedMap<WireAnswer>`.
   - `WireAnswer` is `#[serde(tag = "type", rename_all = "lowercase")]`, with
     `Noul { noul: f64 }`, `Choice { choice: String, confidence: Option<f64>,
     probabilities: DupCheckedMap<f64> }`, and `Score { score: f64, confidence:
     Option<f64>, probabilities: DupCheckedMap<f64> }`.
   - `legend` is ignored through `#[serde(default)] legend: Option<serde_json::Value>`.
   - `usage: Option<{input_tokens: Option<u64>, output_tokens: Option<u64>}>`.
   - A score key that does not parse as `usize` → `Protocol`.
   - A decode error (including a duplicate key) → `Protocol` with the truncated
     message.
3. Map to `RawNamedAnswer`, call `correlate`, and map the usage (`cached_*` stay
   `None`). `DecisionResponse.model` = the wire `model` (a required `String`; missing
   → `Protocol`).
4. `decide` runs `build_request`, then `self.http.post_json(body, &req.model).await`, then `parse_response`.

**Tests:** (module `tests` in `system_one.rs`, pure)
- `request_shapes`
  - setup: triage, `Text("hi")`
  - assert: `state == "hi"`, `questions.is_billing == {"type":"noul",
    "instructions":"Is this a billing issue?"}`, `questions.dept.criteria ==
    {"billing":"payments","tech":null}`, `questions.urgency.criteria ==
    ["low","medium","high: outage"]`
- `request_json_state`
  - setup: `Json({"a":1})`
  - assert: `state == {"a":1}`
- `parse_design_example`
  - setup: `SYSTEM_ONE_TRIAGE_RESPONSE` and the triage questions
  - assert: `noul` → `Predicate{0.97}`, `dept.choice == "billing"` with
    `confidence == 0.99`, and the usage is `{Some(375), Some(0), None, None}`
- `parse_live_probe_fixture`
  - setup: the triage questions and the 3-level fixture `{"0":0.2933117287232892,"1":0.44931357336994154,
    "2":0.25737469790676937}`, `score 0.9640629691834803`
  - assert: the score is preserved exactly, and the probabilities are in level order
- `parse_duplicate_answer_key_is_protocol`
  - setup: a raw body with `"dept"` twice
- `parse_duplicate_probability_key_is_protocol`
  - setup: `SYSTEM_ONE_TRIAGE_RESPONSE` with the `dept` probabilities replaced by the
    raw text `{"billing":0.5,"billing":0.5,"tech":0.0}`
  - assert: `Err(Protocol(_))`
- `parse_missing_answer_is_protocol`
  - assert: the text contains `urgency`
- `parse_non_numeric_score_key_is_protocol`
  - setup: `urgency` replaced by `{"type":"score","score":0.5,"confidence":0.5,
    "probabilities":{"x":0.5,"1":0.25,"2":0.25}}`
  - assert: `Err(Protocol(_))`
- `never_produces_refusal`
  - setup: a body with `"type":"refusal"`
  - assert: `Protocol` (unknown variant)
- command: `cargo test -p camel-component-llm --lib --features decisions decision::system_one`
- expected: fails before, passes after

**Acceptance:**
- The tests above pass.

- Until Task 2.5 wires the factory, the new provider struct has no non-test caller. For this task only, run clippy as `cargo clippy -p camel-component-llm --all-targets --features decisions -- -D warnings -A dead_code`. That must exit 0. Task 2.5 restores the full gate.

- [x] 2.4

### Task 2.5: Feature-gated factory for the HTTP decision providers

**Files:**
- `LLM/Cargo.toml` (modified)
- `LLM/src/decision/factory.rs` (modified)

**Steps:**
1. In `Cargo.toml`, set `all-providers = ["openai", "ollama", "generic-providers",
   "decisions"]`.
2. Use the providers from Tasks 2.3 and 2.4: `OpenaiDecisionProvider::new`, `SystemOneDecisionProvider::new_typesafe`, and `SystemOneDecisionProvider::new_ollama`, each with the global `LlmGlobalConfig.timeout_secs` as `global_timeout`. Under `#[cfg(feature = "decisions")]`, replace the Task 1.6 `UnsupportedCapability` arms for `Openai`, `Typesafe`, and `Ollama` with arms that build these providers.
3. In `factory.rs`, behind `#[cfg(feature = "decisions")]`:
   - For every explicit or required base URL, call
     `crate::provider_factory::validate_llm_url(base, policy)?` before you build the
     provider.
   - `policy` = `AllowInternal` if `config.allow_internal`, else `PublicHttpsOnly`.
   - Keep the `#[cfg(not(feature = "decisions"))]` arms from Task 1.6.

**Tests:** (`factory.rs` tests)
- `factory_rejects_plain_http_public_typesafe` (`#[cfg(feature = "decisions")]`)
  - setup: `base_url = "http://api.typesafe.ai"`, `allow_internal = false`
  - assert: `Err(InvalidRequest(_))`
- `factory_rejects_ollama_loopback_without_allow_internal` (`#[cfg(feature = "decisions")]`)
  - setup: `base_url = "http://127.0.0.1:11434"`
  - assert: `Err(InvalidRequest(_))`
- `factory_builds_all_types_with_allow_internal` (`#[cfg(feature = "decisions")]`,
  `#[tokio::test]`)
  - setup: an openai, typesafe, and ollama entry whose `base_url` is a wiremock
    `server.uri()`, with `allow_internal = true`
  - assert: the map has 3 entries, and `capabilities().max_questions` is `400 / 400 /
    64`. Wiremock got 0 requests (lazy build).
- `factory_rejects_openai_without_feature` (from Task 1.6, still under
  `not(feature)`)
  - command: `cargo test -p camel-component-llm --lib --no-default-features --features mock decision::factory`
- commands: `cargo test -p camel-component-llm --lib --features decisions
  decision::factory`, plus the command above
- expected: fails before, passes after

**Acceptance:**
- The tests above pass.
- `cargo build -p camel-component-llm --no-default-features --features mock` and
  `--features all-providers` both succeed with no warnings.
- `cargo test -p camel-component-llm --features all-providers --test boundary` passes.

- [x] 2.5

### Task 2.6a: HTTP decision codec and status tests (wiremock)

**Files:**
- `LLM/tests/decision_http.rs` (new; `#![cfg(feature = "decisions")]`; `mod common;`)

**Steps:**
1. Write the helper `fn component(providers_toml: &str) -> LlmComponent`. It builds
   `LlmComponent::new` from `allow_internal = true\n` + `providers_toml` +
   `common::decision::TRIAGE_TOML`.
2. Write the helper `async fn run(component: &LlmComponent, uri: &str, body: Body) ->
   Result<Exchange, CamelError>`. It uses the producer pattern in "Shared test
   fixtures" and builds the Exchange with `Exchange::new(Message::new(body))`. Check
   the constructor names in `tests/chat_materialized.rs`.
3. Wiremock: `MockServer::start().await` and `Mock::given(method("POST"))
   .and(path(..)).respond_with(ResponseTemplate::new(..))`. Read requests with
   `server.received_requests().await`. Use no free-port helpers, and no raw HTTP.

**Tests:**
- `same_set_on_every_http_codec`
  - setup: openai (`base_url = server_a.uri()`, POST `/decisions` →
    `OPENAI_TRIAGE_RESPONSE`), typesafe (`base_url = server_b.uri()`, POST
    `/v1/systemone` → `SYSTEM_ONE_TRIAGE_RESPONSE`), ollama (`base_url =
    server_b.uri()`)
  - action: `run` with `llm:decide?provider=<p>&questions=triage` and a text body,
    for each `p`
  - assert: each body has `answers.is_billing.type == "predicate"`,
    `answers.dept.choice == "billing"`, and `answers.urgency.type == "score"`
- `openai_refusal_per_question`
  - setup: `OPENAI_TRIAGE_RESPONSE` with the `dept` answer replaced by
    `{"type":"refusal","name":"dept"}`
  - assert: `answers.dept == {"type":"refusal"}`, and `is_billing` and `urgency` keep
    their types
- `system_one_json_string_body_is_text_state`
  - setup: typesafe, `Body::Json(json!("hello"))`
  - assert: the received request JSON has `"state" == "hello"`
- `openai_json_scalar_array_no_io`
  - setup: `Body::Json(json!([1,2]))`
  - assert: `Err`, and `received_requests` is empty
- `status_table`
  - setup: a loop over `(status, body, check)` with these rows: `(401,"",AuthFailed)`,
    `(403,"",AuthFailed)`, `(404,"",ModelNotFound)`, `(400,"",InvalidRequest)`,
    `(413,"",InvalidRequest)`, `(415,"",InvalidRequest)`, `(422,"",InvalidRequest)`,
    `(429,r#"{"error":{"code":"insufficient_quota"}}"#,QuotaExceeded)`,
    `(429,"",RateLimit)`, `(500,"",ProviderUnavailable)`, and
    `(529,"",ProviderUnavailable)`. Each row uses a fresh `MockServer` and a typesafe
    provider without retry.
  - action: `component.decision_providers()["ts"].decide(DecisionRequest { model:
    "m".into(), state: DecisionState::Text("x".into()), questions: <triage as
    Arc<[DecisionQuestion]>, built in the test> })`
  - assert: `matches!(err, LlmError::<Variant> { .. })` for the row
- `auth_failure_redacted`
  - setup: openai `api_key = "sk-secret-123"`, a 401 response
  - assert: `Err(CamelError::Unauthenticated(_))` through the producer, and
    `sk-secret-123` is in neither the error text nor
    `format!("{:?}", component.config().decision_providers)`
- `redirect_not_followed`
  - setup: server A returns 302 with `Location: {server_b.uri()}/v1/systemone`, and
    server B returns 200 `SYSTEM_ONE_TRIAGE_RESPONSE`. The provider is typesafe on
    server A.
  - assert: `Err`, and server B `received_requests` is empty
- `decide_returns_variants_in_question_order`
  - setup: the openai and typesafe providers from `same_set_on_every_http_codec`
  - action: call `component.decision_providers()[p].decide(..)` directly with the
    triage questions
  - assert: `answers` names are `[is_billing, dept, urgency]`, with the variants
    `Predicate`, `Choice`, `Score`
- `ollama_cap_exceeded_no_io`
  - setup: an ollama provider on wiremock, and an extra set `big` with 65 predicate
    questions `q0..q64`, generated in TOML with a loop
  - action: `create_endpoint("llm:decide?provider=local&questions=big")`
  - assert: `Err`, the text contains `64`, and `received_requests` is empty
- command: `cargo test -p camel-component-llm --features decisions --test decision_http`
- expected: fails before, passes after

**Acceptance:**
- All the tests above pass.
- `cargo xtask lint-no-free-port` and `cargo xtask lint-no-raw-http-test-server`
  exit 0.

- [x] 2.6a

### Task 2.6b: HTTP retry, timeout, and cancellation tests (wiremock)

**Files:**
- `LLM/tests/decision_http_resilience.rs` (new; `#![cfg(feature = "decisions")]`;
  `mod common;`)

**Steps:**
1. Define two helpers in this file. Each integration test file is its own crate.
   - `fn component(providers_toml: &str) -> LlmComponent` builds `LlmComponent::new`
     from `allow_internal = true\n` + `providers_toml` + `common::decision::TRIAGE_TOML`.
   - `async fn run(component: &LlmComponent, uri: &str, body: Body) -> Result<Exchange,
     CamelError>` follows the producer pattern in "Shared test fixtures".
2. Wrap every await on a producer in `tokio::time::timeout`. Use no `sleep`.

**Tests:**
- `retry_after_then_success`
  - setup: typesafe with `network_retry = { enabled = true, max_attempts = 3,
    initial_delay_ms = 10, max_delay_ms = 50 }`. Wiremock returns 429 with header
    `Retry-After: 1`, `.up_to_n_times(1)`, mounted first. A second mock returns 200
    `SYSTEM_ONE_TRIAGE_RESPONSE`.
  - action: `let t0 = Instant::now(); run(...)` under `timeout(10s)`
  - assert: `Ok`, 2 requests received, and `t0.elapsed() >= Duration::from_secs(1)`
- `quota_exhaustion_no_retry`
  - setup: openai with the same retry policy, and a 429 response with body
    `{"error":{"code":"insufficient_quota"}}`
  - assert: `Err`, the text contains `quota exceeded`, and 1 request was received
- `timeout_fires`
  - setup: typesafe `timeout_secs = 1`, no retry, and a 200 response with
    `.set_delay(Duration::from_secs(5))`
  - action: `run(...)` under `timeout(10s)`
  - assert: inner `Err`, the text contains `timeout`, and `elapsed < 4s`
- `cancellation_releases_permit`
  - setup: typesafe `max_concurrency = 1`. A delayed mock (10s,
    `.up_to_n_times(1)`) is mounted first, then a fast 200 mock. Create one endpoint
    and one producer, and `clone()` the producer for the second call.
  - action: `let _ = tokio::time::timeout(Duration::from_millis(200),
    p1.call(ex1)).await;` (dropped), then `tokio::time::timeout(Duration::from_secs(3),
    p2.call(ex2)).await`
  - assert: the second result is `Ok(Ok(_))`
- command: `cargo test -p camel-component-llm --features decisions --test decision_http_resilience`
- expected: fails before, passes after

**Acceptance:**
- All the tests above pass.
- `cargo xtask lint-test-sleep`, `cargo xtask lint-unbounded-wait`, and `cargo xtask
  lint-no-free-port` exit 0.

- [x] 2.6b

### Task 2.7a: Live Ollama test and component docs

**Files:**
- `LLM/tests/decision_ollama_live.rs` (new)
- `docs/src/components/llm.md` (modified)

**Steps:**
1. Write `decision_ollama_live.rs`:
   - `#![cfg(feature = "decisions")]`, `mod common;`, and one `#[tokio::test]
     #[ignore = "slow test: requires local Ollama with tev1:0.8b"]` test,
     `ollama_tev1_decides_triage`.
   - Config: `allow_internal = true`, `[decision_providers.local]` with `type =
     "ollama"`, `base_url = "http://127.0.0.1:11434"`, `default_model = "tev1:0.8b"`,
     plus `common::decision::TRIAGE_TOML`.
   - Action: build the producer for `llm:decide?provider=local&questions=triage` and
     call it with `Body::Text("My invoice was charged twice, please refund")` inside
     `tokio::time::timeout(Duration::from_secs(120))`.
   - Assert: `answers.is_billing.type == "predicate"`, `answers.dept.type ==
     "choice"`, `answers.urgency.type == "score"`, and every `probability` and
     `confidence` number is in `[0,1]`. Assert nothing about which answer is
     correct.
2. In `docs/src/components/llm.md`, add a section `## Typed decisions (llm:decide)`
   with these parts:
   - One sentence of intent: typed answers with probabilities. Then a sentence that
     a typed answer can still be wrong.
   - A TOML block with `[decision_providers.*]` entries of all four types (`type =
     "openai"`, `type = "typesafe"`, `type = "ollama"`, and `type = "mock"`), plus
     the triage question set.
   - The URI `llm:decide?questions=<set>[&provider=<name>][&model=<id>]`, with a note
     that `provider` names a `decision_providers` key, not a `providers` key, and
     that the chat parameters are rejected.
   - The state mapping table from design.md.
   - The output JSON schema, with `meets_threshold` and `meets_min_confidence`.
   - A warning: a refusal has no `meets_*` key, so a `== true` route goes to
     `otherwise`.
   - The headers table, including `CamelLlmCachedTokensIn` and
     `CamelLlmCacheWriteTokensIn`, and the absence of a cost header.
   - One YAML route example with `choice` / `simple:
     "${body.answers.dept.choice} == 'billing'"`.
   - The feature `decisions` (included in `all-providers`), and `allow_internal =
     true` for local Ollama.
   - A note that Simple has no float comparisons (rc-8ww5h). Use the `meets_*`
     booleans, or Rhai/JSONPath for raw probabilities.

**Tests:**
- `ollama_tev1_decides_triage`. It is ignored by default.
  - command: `cargo test -p camel-component-llm --features decisions --test decision_ollama_live`
  - expected: `1 ignored`
  - opt-in: `... -- --ignored`, which passes against a local Ollama with `tev1:0.8b`
- Docs check
  - command: `for t in openai typesafe ollama mock; do grep -q "type = \"$t\"" docs/src/components/llm.md || echo MISSING $t; done`
  - expected: no output
- `cd docs && mdbook test` exits 0 (if `mdbook` is not installed, record N/A).

**Acceptance:**
- The commands above meet their expectations.
- The docs make no claim that typed answers are correct or free of hallucinations.

- [x] 2.7a

### Task 2.7b: CONTEXT, CONTEXT-MAP, and the ADR-0020 addendum

**Files:**
- `LLM/CONTEXT.md` (modified)
- `CONTEXT-MAP.md` (modified)
- `docs/adr/0020-llm-component-provider-adapter-boundary.md` (modified)

**Steps:**
1. In `LLM/CONTEXT.md`:
   - Add glossary entries `DecisionProvider`, `Question set`, `Decision answer`, and
     `System One codec`, each with one definition paragraph and an `_Avoid_:` line.
   - Under "Breaking changes (0.x)", add these bullets:
     - `LlmOperation` gains `Decide` and becomes `#[non_exhaustive]`.
     - `LlmGlobalConfig` gains `default_decision_provider`, `decision_providers`, and
       `decision_question_sets`.
     - `LlmEndpointConfig` gains `questions`.
     - `LlmComponent::new` now runs `validate()`.
   - Add these log-policy rows:
     - "Decision provider construction failed": `error!`
     - "Decide producer error": `warn!`
2. In `CONTEXT-MAP.md`:
   - Extend the LLM Component line with ", plus typed decisions (OpenAI Decisions,
     TypeSafe, Ollama System One) through a separate DecisionProvider".
   - Add a glossary bullet `**DecisionProvider**` that cites "ADR-0020 addendum
     2026-11-03", in the style of the existing `LlmProvider` bullet.
   - Append "; the 2026-11-03 addendum adds the decision provider seam" to the
     ADR-0020 index line.
3. Append to ADR-0020 a section `## Addendum 2026-11-03: Decision provider seam` that
   records the following points, in prose:
   - The separate `DecisionProvider` trait, and the reasons: no streaming semantics,
     and no stubs on chat adapters.
   - Direct reqwest adapters for OpenAI Decisions and System One (TypeSafe and
     Ollama). No genai version supports decisions; 0.6.5, 0.7.0-rc.8, and 0.7.2 were
     checked on 2026-11-03.
   - The replacement trigger: when a released genai supports decisions, add a genai
     `DecisionProvider` in an allowlisted adapter file. Config `type` values and
     routes stay unchanged.
   - The shared `provider/http_errors.rs` has no SDK import.
   - Bd: rc-3lnv3.

**Tests:**
- `cargo xtask lint-context-citations` exits 0.
- `grep -n "Addendum 2026-11-03" docs/adr/0020-llm-component-provider-adapter-boundary.md`
  returns 1 line.

**Acceptance:**
- The commands above pass.
- `cargo test -p camel-component-llm --features all-providers` passes.

- [x] 2.7b
