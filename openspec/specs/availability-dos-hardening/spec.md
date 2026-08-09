# availability-dos-hardening Specification

## Purpose
TBD - created by archiving change audit-fix-availability-dos. Update Purpose after archive.
## Requirements
### Requirement: gRPC accept-loop backoff on persistent accept errors

The `run_grpc_server` accept-loop SHALL apply exponential backoff when
`listener.accept()` returns an error, using `camel_api::backoff::BackoffState`
with `BackoffConfig { initial_delay: 10ms, multiplier: 2.0, max_delay: 5s }`.
On a successful accept, the backoff state SHALL reset.

**Accept-flap non-goal:** Under alternating `Err/Ok/Err/Ok` (accept-flap),
reset-on-success keeps backoff at 10ms. This scenario is an explicit
non-goal — the server IS making progress, just slowly. A decay-on-success
fix would require a different mechanism and is filed as follow-up if
observed in production.

#### Scenario: accept error triggers backoff sleep

- **GIVEN** a gRPC server with a `TcpListener` whose `accept()` returns `Err` persistently
- **WHEN** the accept-loop encounters consecutive accept errors
- **THEN** the loop sleeps for an exponentially increasing duration between retries (starting at 10ms, doubling each time, capped at 5s)

#### Scenario: successful accept resets backoff

- **GIVEN** a gRPC server that had N consecutive accept errors followed by a successful accept
- **WHEN** the next accept error occurs
- **THEN** the backoff delay starts again at 10ms (counter was reset by the successful accept)

#### Scenario: backoff does not delay normal operation

- **GIVEN** a gRPC server with no accept errors
- **WHEN** connections arrive normally
- **THEN** no backoff sleep is applied (the happy path is unchanged)

### Requirement: LLM producer rejects oversized header JSON payloads

The `LlmProducer::build_chat_request` SHALL check the serialized byte
size of each JSON-bearing exchange header (`CamelLlmMessages`,
`CamelLlmTools`, `CamelLlmToolChoice`) before deserializing it. If the
serialized size exceeds `max_header_json_bytes` (default 64 KB =
65_536 bytes), the producer SHALL return
`LlmError::InvalidRequest` with a message identifying the offending
header, WITHOUT attempting deserialization.

The `max_header_json_bytes` threshold SHALL be configured via
`LlmGlobalConfig` (following the `max_prompt_bytes` precedent), with a
`#[serde(default = "default_max_header_json_bytes")]` = 65_536, and
threaded through `endpoint.rs` to `LlmProducer` via the builder pattern
(`.with_max_header_json_bytes()`), NOT as a new positional argument to
`LlmProducer::new`.

#### Scenario: oversized CamelLlmMessages header rejected

- **GIVEN** an LLM producer with `max_header_json_bytes = 65536` (default)
- **WHEN** an exchange arrives with a `CamelLlmMessages` header whose JSON serialization exceeds 64 KB
- **THEN** the producer returns `Err(LlmError::InvalidRequest(...))` mentioning "CamelLlmMessages" and "max_header_json_bytes", without calling `serde_json::from_value`

#### Scenario: oversized CamelLlmTools header rejected

- **GIVEN** an LLM producer with `max_header_json_bytes = 65536` (default)
- **WHEN** an exchange arrives with a `CamelLlmTools` header whose JSON serialization exceeds 64 KB
- **THEN** the producer returns `Err(LlmError::InvalidRequest(...))` mentioning "CamelLlmTools" and "max_header_json_bytes", without calling `serde_json::from_value`

#### Scenario: normal-sized headers accepted

- **GIVEN** an LLM producer with default config
- **WHEN** an exchange arrives with `CamelLlmMessages` containing 10 small messages (total < 64 KB)
- **THEN** the producer deserializes normally and builds the chat request successfully

#### Scenario: custom max_header_json_bytes threshold respected

- **GIVEN** an LLM producer configured with `max_header_json_bytes = 1024`
- **WHEN** an exchange arrives with a `CamelLlmMessages` header of 2 KB
- **THEN** the producer rejects it with `LlmError::InvalidRequest(...)` (custom threshold exceeded)

