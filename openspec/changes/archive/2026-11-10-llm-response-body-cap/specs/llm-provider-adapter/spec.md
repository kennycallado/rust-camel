## ADDED Requirements

### Requirement: Response payload caps bound chat and embed reads

The LLM provider adapter SHALL cap accepted response payload growth per path:
chat (streamed and materialized) at `MAX_CHAT_RESPONSE_BYTES` (8 MiB of UTF-8
text bytes), embed at `MAX_EMBED_RESPONSE_BYTES` (64 MiB of decoded f32
vector bytes). Streaming enforcement SHALL apply to genai-backed providers
(the genai adapter stream loop serves openai, ollama, and generic providers);
materialized enforcement SHALL apply in the producer accumulation loop for
all providers. Direct streaming from non-genai providers (the test mock,
custom adapters) through the producer passthrough is out of scope and SHALL
be recorded as residual. Embed enforcement SHALL happen at the Camel
conversion boundary (genai reads the embed JSON before Camel sees data).
The limit is inclusive: a payload of exactly the cap
is accepted; one byte more is rejected. A breach SHALL fail with
`LlmError::ResponseTooLarge` naming the path, the cap, and the attempted
cumulative counted bytes (UTF-8 byte counting for text, f32 byte counting for
vectors, overflow-safe accumulation); it SHALL NOT be classified retryable.
These caps are safety budgets on accepted payloads — they are not wire-body
or process-memory guarantees, and genai SDK-internal buffering (SSE lines,
oversized individual events, captured tool arguments, the embed JSON body,
genai error bodies) stays out of scope. A response at or below the cap SHALL
be delivered byte-identically to the pre-cap behavior. `decision/http.rs`'s
`read_capped` and `MAX_DECISION_RESPONSE_BYTES` SHALL remain unchanged.

#### Scenario: under-cap chat response completes byte-identically

- **GIVEN** a staged provider whose streamed completion totals less than the
  chat cap
- **WHEN** the materialized chat path consumes the stream
- **THEN** the accumulated text equals the fixture exactly and no cap error
  is raised

#### Scenario: exactly-cap payload is accepted

- **GIVEN** a streamed or materialized payload whose UTF-8 byte count equals
  `MAX_CHAT_RESPONSE_BYTES` exactly
- **WHEN** the cap check runs on each chunk
- **THEN** the payload is accepted in full and no cap error is raised

#### Scenario: cap-plus-one payload is rejected

- **GIVEN** a payload whose byte count is `MAX_CHAT_RESPONSE_BYTES + 1`
- **WHEN** the breaching chunk is counted
- **THEN** the call fails with `ResponseTooLarge` naming the path and cap,
  with `read_bytes` reporting the attempted cumulative count

#### Scenario: over-cap streamed chat aborts while reading

- **GIVEN** a staged provider streaming an SSE body whose accepted text
  exceeds the chat cap
- **WHEN** the genai stream loop counts forwarded bytes past the cap
- **THEN** the stream terminates with `ResponseTooLarge` naming
  `chat_stream`, the cap value, and the counted bytes
- **AND** no chunk that would push the count past the cap is forwarded, and
  no `Finished` event is emitted

#### Scenario: oversized first delta forwards nothing

- **GIVEN** a stream whose first chunk alone exceeds the chat cap
- **WHEN** the stream loop checks the first chunk
- **THEN** no delta is forwarded and the stream terminates with
  `ResponseTooLarge`

#### Scenario: multibyte text is counted by bytes not chars

- **GIVEN** a stream of multibyte UTF-8 chunks (e.g. CJK text) whose byte
  total exceeds the cap while its char count stays below it
- **WHEN** the cap counts bytes
- **THEN** the breach fires on byte count and the error reports byte counts

#### Scenario: over-cap materialized accumulation aborts before storing

- **GIVEN** a provider (mock) emitting deltas whose total exceeds the chat cap
- **WHEN** the materialized loop in `producer.rs` checks the accumulated
  byte count before appending
- **THEN** the call fails with `ResponseTooLarge` naming
  `chat_materialized` and the cap, and no partial `MaterializedResult` is
  produced

#### Scenario: producer consumer observes a Camel error on breach

- **GIVEN** a materialized chat exchange whose payload breaches the cap
- **WHEN** the producer surfaces the failure to the exchange
- **THEN** the caller observes a `CamelError` (mapped from
  `ResponseTooLarge`), not a truncated success

#### Scenario: deltas delivered before the breach are not rolled back

- **GIVEN** a streaming chat call that forwarded several deltas below the cap
- **WHEN** the next delta breaches the cap and the stream aborts with
  `ResponseTooLarge`
- **THEN** the caller retains the already-delivered deltas, and the terminal
  error is the cap error

#### Scenario: streaming producer consumer observes deltas then a terminal Camel error

- **GIVEN** a genai-backed streaming chat exchange whose payload breaches the
  cap mid-stream, with several deltas already delivered
- **WHEN** the breach terminates the returned streaming body
- **THEN** the caller received the earlier deltas, then a terminal
  `CamelError::ProcessorError` whose text names the path, the cap, and the
  counted bytes, and no `Finished` event is emitted

#### Scenario: under-cap streaming is unchanged

- **GIVEN** a streamed completion below the cap with deltas of any size
- **WHEN** the caller consumes the stream
- **THEN** delta content, ordering, and completion semantics are identical to
  the pre-cap behavior

#### Scenario: over-cap embed aborts at the Camel boundary

- **GIVEN** a staged embed provider whose decoded embeddings exceed the embed
  cap (exercised via the test cap override)
- **WHEN** `embed()` sums the decoded vector bytes before building
  `EmbedResponse`
- **THEN** the call fails with `ResponseTooLarge` naming `embed`, the cap,
  and the counted bytes, and no `EmbedResponse` is returned to the caller

#### Scenario: cumulative multi-vector embed sum is checked

- **GIVEN** an embed response with multiple vectors whose individual sizes
  are small but whose cumulative byte sum exceeds the cap
- **WHEN** `embed()` accumulates the per-vector byte counts with
  overflow-safe arithmetic
- **THEN** the breach fires on the cumulative sum, not any single vector

#### Scenario: under-cap embed response is byte-identical

- **GIVEN** a staged embed provider returning vectors within the cap
- **WHEN** `embed()` runs
- **THEN** the returned embeddings match the fixture exactly and no cap error
  is raised

#### Scenario: cap error is not retryable

- **GIVEN** a `ResponseTooLarge` error
- **WHEN** the retry loop consults `is_retryable`
- **THEN** the error is not retried

#### Scenario: cap breach never truncates silently

- **GIVEN** any capped path whose input breaches its cap
- **WHEN** the cap check fires
- **THEN** the caller observes an error result — never a truncated success,
  and the decision path's `read_capped` behavior is untouched
