# llm-decisions Specification

## Purpose
TBD - created by archiving change llm-decisions. Update Purpose after archive.
## Requirements
### Requirement: Decision providers implement one project-owned contract

The system SHALL expose a `DecisionProvider` trait in `camel-component-llm` that is
separate from `LlmProvider`. The `openai`, `typesafe`, `ollama`, and `mock` decision
providers SHALL implement it. Request, answer, usage, and capability types SHALL be
Camel-owned and use named fields, and SHALL NOT expose vendor payload types. Files
under `src/decision/` SHALL NOT import genai. The existing `tests/boundary.rs`
import scan enforces this, and doc comments may mention genai. The `LlmProvider` trait SHALL NOT
change.

#### Scenario: Same question set on every HTTP codec

- **GIVEN** a question set with one predicate, one choice, and one score question, and
  wiremock fixtures for the OpenAI and System One wire formats
- **WHEN** the `openai`, `typesafe`, and `ollama` providers each answer the set
- **THEN** each result is a `DecisionResponse` with three `NamedAnswer`s in question
  order, with the variants `Predicate`, `Choice`, and `Score`

#### Scenario: Decision code stays outside the SDK boundary

- **GIVEN** the crate sources
- **WHEN** `cargo test -p camel-component-llm --test boundary` runs
- **THEN** the test passes, and no file under `src/decision/` has a genai `use` or
  path import

### Requirement: Decision configuration is validated operator configuration

The system SHALL read decision providers from `decision_providers`, the default
from `default_decision_provider`, and question sets from `decision_question_sets`.
Each question set SHALL be an ordered `questions` array. Provider types SHALL be
`openai` (`api_key`, `default_model`, optional `base_url`), `typesafe` (`api_key`,
`default_model`, optional `base_url`), `ollama` (`base_url`, `default_model`), and
`mock`. The three HTTP types SHALL also accept the optional `timeout_secs`,
`max_concurrency`, and `network_retry`. Unknown fields SHALL be rejected.
`LlmGlobalConfig::validate` SHALL enforce these rules:

- 1–400 questions per set.
- Names match `^[A-Za-z0-9_-]{1,64}$` and are unique in the set.
- `instructions` is not blank.
- A choice has 2–255 unique, non-blank values.
- A score has 2–10 levels with non-blank labels.
- `threshold` is valid on a predicate only. `min_confidence` is valid on a choice or
  score only. Both are finite values in `[0, 1]`.
- `api_key`, `base_url`, and `default_model` are not blank.
- Mock `predicate_probability` is finite and in `[0, 1]`.
- Every mock `refuse` entry is a syntactically valid question name.
- `timeout_secs` and `max_concurrency` are not 0.
- `choices` appears only on a choice question, and `levels` only on a score question.

The question config SHALL be one flat struct with optional `choices`, `levels`,
`threshold`, and `min_confidence`. A misplaced field therefore deserializes and then
fails in `validate()`. `LlmComponent::new` SHALL call `validate()`, as `LlmBundle`
already does.

Provider `Debug` output SHALL redact `api_key`.

#### Scenario: Duplicate question name

- **GIVEN** a set `triage` with two questions named `dept`
- **WHEN** the configuration is validated
- **THEN** validation fails with `CamelError::Config` whose text contains `triage`
  and `dept`

#### Scenario: Choice with one option

- **GIVEN** a choice question with one choice
- **WHEN** the configuration is validated
- **THEN** validation fails with `CamelError::Config` whose text contains `2..=255`

#### Scenario: Threshold on a choice question

- **GIVEN** a choice question with `threshold = 0.5`
- **WHEN** the configuration is validated
- **THEN** validation fails with `CamelError::Config` whose text contains `threshold`

#### Scenario: Unknown field

- **GIVEN** an `ollama` provider entry with an `api_key` field
- **WHEN** the TOML is deserialized
- **THEN** deserialization fails

### Requirement: Mock decision provider is deterministic

The `mock` provider SHALL accept the optional fields `default_model` (default
`mock-decision`), `predicate_probability` (default `1.0`), `refuse` (a list of
question names, default empty), and `error_message`. Its answers are fixed:

- `error_message` set → `LlmError::Provider` that contains the message.
- A question named in `refuse` → `Refusal`.
- Predicate → `predicate_probability`.
- Choice → the first declared value. That value gets probability 1.0 and every other
  value 0.0. Confidence 1.0.
- Score → `score = 0.0`. Level 0 gets probability 1.0 and every other level 0.0.
  Confidence 1.0.
- All usage fields are `None`.

The mock SHALL report `max_questions = 400`.

#### Scenario: Mock answers a mixed set

- **GIVEN** a mock with `predicate_probability = 0.25` and `refuse = ["urgency"]`, and
  the set `is_billing` (predicate), `dept` (choice: billing, tech), `urgency` (score)
- **WHEN** `decide` runs
- **THEN** the answers are `Predicate{0.25}`; `Choice{choice:"billing", confidence:1.0,
  probabilities:[billing 1.0, tech 0.0]}`; and `Refusal`

### Requirement: Decide endpoint fails fast before network I/O

The system SHALL accept `llm:decide?questions=<set>[&provider=<name>][&model=<id>]`.
`LlmEndpointConfig::from_uri` SHALL reject these parameters with `InvalidUri`:
`stream`, `temperature`, `max_tokens`, and `system_prompt` on `decide`, and
`questions` on `chat` and `embed`. Endpoint creation SHALL reject each of these
cases with `InvalidUri`:

- A missing `questions` parameter or an unknown set.
- A missing or unknown decision provider. The provider comes from the `provider`
  parameter or from `default_decision_provider`.
- A set larger than `capabilities().max_questions`.

None of these cases SHALL perform network I/O. `LlmBundle` and `LlmComponent::new`
SHALL build both provider maps. `LlmComponent::from_parts` SHALL produce an empty
decision map unless `with_decision_providers` is called.

#### Scenario: Set exceeds the Ollama cap

This scenario requires the `decisions` feature.

- **GIVEN** an `ollama` provider whose `base_url` is a wiremock server
  (`allow_internal = true`), and a set `big` with 65 questions
- **WHEN** `create_endpoint("llm:decide?provider=local&questions=big")` runs
- **THEN** it fails with `InvalidUri` whose text contains `64`, and wiremock got 0
  requests

#### Scenario: Unknown question set

- **GIVEN** a configured mock decision provider set as `default_decision_provider`
- **WHEN** `create_endpoint("llm:decide?questions=nope")` runs
- **THEN** it fails with `InvalidUri` whose text contains `nope`

#### Scenario: Chat parameter on decide

- **GIVEN** the URI `llm:decide?questions=triage&temperature=0.2`
- **WHEN** `LlmEndpointConfig::from_uri` runs
- **THEN** it fails with `InvalidUri` whose text contains `temperature`

### Requirement: Decision state comes from the Exchange body

The decide producer SHALL map the body to the decision state before any I/O, as
follows:

- `Text`, `Xml`, UTF-8 `Bytes`, and `Json(String)` become text state.
- A `Json` object or array becomes JSON state.
- A `Json` number, bool, or null; `Empty`; `Stream`; and invalid UTF-8 return
  `LlmError::InvalidRequest`.

The size SHALL be the text byte length or the serialized JSON byte length. A state
larger than `max_prompt_bytes` SHALL return `InvalidRequest`. The OpenAI adapter SHALL
also reject, before any I/O, a JSON array that contains a non-object element.

#### Scenario: JSON number body

- **GIVEN** a mock decision endpoint and the body `Body::Json(42)`
- **WHEN** the producer runs
- **THEN** it fails with a `CamelError` whose text contains `invalid request`

#### Scenario: JSON array of scalars on OpenAI

- **GIVEN** an `openai` provider on wiremock and the body `Body::Json([1, 2])`
- **WHEN** `decide` runs
- **THEN** it returns `InvalidRequest`, and wiremock got 0 requests

#### Scenario: JSON string becomes text state

- **GIVEN** a System One provider on wiremock and the body `Body::Json("hello")`
- **WHEN** the producer runs
- **THEN** the request body sent has `"state": "hello"`

### Requirement: Answers are correlated by name and validated

The OpenAI codec SHALL decode these fields, as the official reference documents
them:

- `answers[]` items: `type`, `name`.
- Predicate: `probability`.
- Choice: `choice`, `confidence`, and `probabilities[{value, probability}]`.
- Score: `score`, `confidence`, and `probabilities[{label, value, probability}]`, where
  `value` is the 0-based level.
- Refusal: `type` only.

The System One codec SHALL decode these fields:

- `answers{name: …}`.
- Noul: `noul`.
- Choice: `choice`, `confidence`, and `probabilities{value: p}`.
- Score: `score`, `confidence`, `legend` (ignored), and `probabilities{"<level>": p}`.

A missing `confidence` on a choice or score SHALL return `Protocol`. Nothing is
derived. A response body larger than 8 MiB SHALL return `Protocol`.

The system SHALL match answers to questions by name, not by array position. Each of
these cases SHALL return `LlmError::Protocol`:

- A missing, duplicate, or unknown answer name. On System One, a duplicate is a
  repeated JSON object key, and a duplicate-rejecting decoder detects it.
- A kind mismatch.
- A non-finite value, or a probability or confidence outside `[0, 1]` (tolerance
  `1e-9`).
- A choice that is not declared.
- A choice distribution whose key set is not the declared set, or that has a
  duplicate key or value.
- A score outside `[0, levels-1]`, or a score distribution whose levels are not
  exactly `0..levels`.
- A distribution sum that differs from 1 by more than `0.01`.

A malformed answer SHALL NOT become a false predicate. Probability values SHALL be
preserved as received, without clamping. On OpenAI, the wire score labels SHALL be
ignored, and the declared labels used.

#### Scenario: OpenAI answers out of order

- **GIVEN** an OpenAI response whose `answers` list is in reverse question order
- **WHEN** it is decoded
- **THEN** each `NamedAnswer` holds the answer with the matching `name`, in question
  order

#### Scenario: Missing answer

- **GIVEN** a System One response with no entry for `urgency`
- **WHEN** it is decoded
- **THEN** the result is `Protocol` whose text contains `urgency`

#### Scenario: Duplicate System One key

- **GIVEN** a System One response body whose `answers` object has the key `dept`
  twice
- **WHEN** it is decoded
- **THEN** the result is `Protocol`

#### Scenario: Probability out of range

- **GIVEN** a predicate answer with probability `1.5`
- **WHEN** it is decoded
- **THEN** the result is `Protocol`

#### Scenario: Fractional score is preserved

- **GIVEN** a System One score answer with `score = 0.964` and three levels whose
  probabilities are `{"0":0.29,"1":0.45,"2":0.26}`
- **WHEN** it is decoded
- **THEN** `score == 0.964`, and the probabilities are `[(0,0.29),(1,0.45),(2,0.26)]`

### Requirement: Refusal is a per-question outcome

The OpenAI adapter SHALL map an answer of type `refusal` to `DecisionAnswer::Refusal`
for that question only. The other answers SHALL decode normally. A refusal SHALL NOT
be an error and SHALL NOT be a predicate. The System One adapter SHALL NOT produce
`Refusal`.

#### Scenario: One refused question

- **GIVEN** an OpenAI response with `{"type":"refusal","name":"dept"}` and two
  normal answers
- **WHEN** the producer runs
- **THEN** `answers.dept` is `{"type":"refusal"}`, and the other two answers are
  present and typed

### Requirement: Decision output is a stable JSON body

The producer SHALL replace the body with
`{"model": <string>, "answers": {<name>: <answer>}}`. Each answer has one of these
shapes:

- Predicate: `{"type":"predicate","probability"}`.
- Choice: `{"type":"choice","choice","confidence","probabilities":{<value>:<p>}}`.
- Score: `{"type":"score","score","confidence","probabilities":[{"level","label","probability"}]}`.
- Refusal: `{"type":"refusal"}`.

`meets_threshold` (`probability >= threshold`) SHALL appear on a predicate only when
`threshold` is configured. `meets_min_confidence` (`confidence >= min_confidence`)
SHALL appear on a choice or score only when `min_confidence` is configured. A refusal
SHALL NOT have either key.

#### Scenario: Simple predicate reads the threshold boolean

- **GIVEN** a mock provider with `predicate_probability = 1.0`, a predicate
  `is_billing` with `threshold = 0.8`, and the route
  `direct:in → llm:decide?provider=fake&questions=triage →
  validate("${body.answers.is_billing.meets_threshold} == true") → mock:billing`
- **WHEN** one Exchange with a text body is sent to `direct:in`
- **THEN** `mock:billing` receives exactly one Exchange within 5 seconds

#### Scenario: Choice routing on the chosen value

- **GIVEN** a route `llm:decide → choice().when(body.answers.dept.choice == "billing") →
  mock:billing, otherwise → mock:other` with the mock provider
- **WHEN** one Exchange is sent
- **THEN** `mock:billing` gets 1 Exchange, and `mock:other` gets 0

#### Scenario: No threshold, no boolean

- **GIVEN** a predicate question without `threshold`
- **WHEN** the producer runs
- **THEN** the predicate answer object has no `meets_threshold` key

### Requirement: Decision usage is reported, not synthesized

The producer SHALL set `CamelLlmProvider`, `CamelLlmModel`, and
`CamelLlmUsageAvailable = true`. It SHALL set `CamelLlmTokensIn`, `CamelLlmTokensOut`,
`CamelLlmCachedTokensIn`, and `CamelLlmCacheWriteTokensIn` only when the provider
reports that value. The producer SHALL NOT set `CamelLlmEstimatedCostUsd` for decide.

#### Scenario: OpenAI cached usage

- **GIVEN** an OpenAI usage object with `input_tokens = 2000`,
  `cached_tokens = 1500`, `cache_write_tokens = 0`, and `output_tokens = 0`
- **WHEN** the producer runs
- **THEN** the headers are `CamelLlmTokensIn = 2000`, `CamelLlmCachedTokensIn = 1500`,
  `CamelLlmCacheWriteTokensIn = 0`, and `CamelLlmTokensOut = 0`

#### Scenario: System One usage

- **GIVEN** a System One usage object `{"input_tokens":375,"output_tokens":0}`
- **WHEN** the producer runs
- **THEN** `CamelLlmTokensIn = 375` and `CamelLlmTokensOut = 0`, and
  `CamelLlmCachedTokensIn` is absent

### Requirement: HTTP failures map to the LlmError taxonomy

`map_status`, `is_insufficient_quota`, `parse_retry_after`, and `truncate_body` SHALL
move unchanged to `provider/http_errors.rs`, which does not import genai. The genai
adapter SHALL use them from there, and its mapping SHALL behave as before. The
decision adapters SHALL map 413 and 415 to `InvalidRequest`, then call `map_status`
for every other status:

| Status | Result | Retryable |
|---|---|---|
| 401/403 | `AuthFailed` | no |
| 404 | `ModelNotFound` | no |
| 400/422 | `InvalidRequest` | no |
| 429 with an `insufficient_quota` code or type | `QuotaExceeded` | no |
| other 429 | `RateLimit { retry_after }` (delta-seconds `Retry-After`) | yes |
| 5xx, including 529 | `ProviderUnavailable` | yes |
| reqwest timeout | `Timeout(configured)` | yes |

The decision path SHALL pass the whole capped error body to `map_status`, so the
`insufficient_quota` check sees valid JSON. Error text SHALL hold at most 200 body
bytes; `map_status` and `truncate_body` enforce this. Error text and `Debug`
output (config and adapter) SHALL NOT contain the API key. The HTTP status scenarios require the `decisions`
feature.

The decision adapters SHALL build their client only with
`hardened_http_client_with_pinning`: no redirects, `no_proxy`, and
`resolve_to_addrs`. A per-request `.timeout(configured)` SHALL be set.

#### Scenario: Throttled and retried with Retry-After

- **GIVEN** a `typesafe` provider on wiremock with `network_retry` enabled
  (`max_attempts = 3`). The first response is a synthetic 429 with `Retry-After: 1`;
  TypeSafe documents no Retry-After header. The next response is a valid 200.
- **WHEN** the producer runs in real time
- **THEN** it succeeds, wiremock got 2 requests, and the elapsed time is at least
  1 second

#### Scenario: Quota exhaustion fails fast

- **GIVEN** an `openai` provider with retry enabled, and a 429 response with body
  `{"error":{"code":"insufficient_quota"}}`
- **WHEN** `decide` runs through the producer
- **THEN** the result is an error whose text contains `quota exceeded`, and wiremock
  got 1 request

#### Scenario: Status table

- **GIVEN** a wiremock that returns each status in the table (`529` included)
- **WHEN** an adapter `decide` call runs once per status
- **THEN** each result is the `LlmError` variant that the table names

#### Scenario: Auth failure is redacted

- **GIVEN** an `openai` provider with `api_key = "sk-secret-123"`, and a 401 response
- **WHEN** `decide` runs
- **THEN** the result is `AuthFailed`, and `sk-secret-123` is in none of these: the
  error text, `format!("{:?}")` of the config, or `format!("{:?}")` of the adapter

#### Scenario: Redirect is not followed

- **GIVEN** a `typesafe` provider on wiremock (`allow_internal = true`) that returns
  302 with `Location` pointing to a second wiremock server
- **WHEN** `decide` runs
- **THEN** the result is an error, and the second server got 0 requests

#### Scenario: Timeout

The per-attempt and total timeouts are both 1 second here, so this scenario proves
that a timeout fires. It does not tell the two timeouts apart.

- **GIVEN** an endpoint with `timeout_secs = 1`, no `network_retry`, and a wiremock
  response delayed 5 seconds
- **WHEN** the producer runs
- **THEN** the result is `CamelError::ProcessorError` whose text contains `timeout`,
  and it returns in less than 4 seconds

#### Scenario: Cancellation releases the permit

- **GIVEN** an endpoint with `max_concurrency = 1`. Wiremock delays the first response
  10 seconds (up to 1 time), and the next response is a fast 200.
- **WHEN** the first producer call is dropped by a 200 ms `tokio::time::timeout`, and
  then a second call runs under a 3-second `tokio::time::timeout`
- **THEN** the second call returns `Ok`

### Requirement: Decision adapters are feature gated and SSRF hardened

The HTTP adapters SHALL compile only with the `decisions` feature. `all-providers`
SHALL include it. Without the feature, an `openai`, `typesafe`, or `ollama` config
SHALL parse, and provider-map construction SHALL fail with `UnsupportedCapability`
naming `decisions`. At startup, the factory SHALL call `validate_llm_url` on every
explicit or required `base_url`. On the first `decide`, before the lazy client build,
the adapter SHALL call `validate_llm_url_pinned` and SHALL pin DNS for a domain
`base_url`. Default public URLs SHALL NOT be pinned.

#### Scenario: Plain HTTP public URL rejected

- **GIVEN** a `typesafe` provider with `base_url = "http://api.typesafe.ai"` and
  `allow_internal = false`
- **WHEN** the decision provider map is built
- **THEN** it fails with `InvalidRequest`

#### Scenario: Ollama loopback without allow_internal

- **GIVEN** an `ollama` provider with `base_url = "http://127.0.0.1:11434"` and
  `allow_internal = false`
- **WHEN** the decision provider map is built
- **THEN** it fails with `InvalidRequest`, and no request is sent

#### Scenario: Build without the decisions feature

- **GIVEN** `cargo test -p camel-component-llm --no-default-features --features mock`
- **WHEN** an `openai` decision provider config is built into a provider map
- **THEN** the result is `UnsupportedCapability` whose text contains `decisions`

### Requirement: Live Ollama decision test is opt-in

The crate SHALL have `tests/decision_ollama_live.rs` with a test marked
`#[ignore = "slow test: requires local Ollama with tev1:0.8b"]`. The test calls a
route on `http://127.0.0.1:11434` with `allow_internal = true` and asserts that each
answer kind decodes. It runs only through `--ignored` with the `decisions` feature.

#### Scenario: Live test is skipped by default

- **GIVEN** `cargo test -p camel-component-llm --features decisions`
- **WHEN** the test suite runs
- **THEN** the live test is reported as ignored

### Requirement: Decision documentation and ADR record the seam

The system SHALL document the following:

- `docs/src/components/llm.md`: the decide configuration for all four provider types,
  the URI, the output schema, the refusal fall-through warning, the new headers, and
  a routing example.
- The crate `CONTEXT.md` and `CONTEXT-MAP.md`: the `DecisionProvider`, question set,
  and decision answer terms, plus the `LlmOperation` break.
- An ADR-0020 addendum: the direct-HTTP decision seam and the genai replacement
  trigger.

The docs SHALL NOT claim that typed answers are factually correct.

#### Scenario: Docs mention every provider type

- **GIVEN** the updated `docs/src/components/llm.md`
- **WHEN** a reader searches it for `type = "openai"`, `type = "typesafe"`,
  `type = "ollama"`, and `type = "mock"` under `decision_providers`
- **THEN** each one appears

