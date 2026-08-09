## ADDED Requirements

### Requirement: Char-semantics log truncation

The log component SHALL truncate exchange body strings to a maximum number
of characters (not bytes), preventing panics when multibyte UTF-8 sequences
are present.

#### Scenario: Truncate lands inside a multibyte character

- **GIVEN** a LogProducer with `maxChars=5`
- **WHEN** the body string is `"café_x"` (6 chars including multibyte é) or `"日本語测试"` (5 chars, 15 bytes)
- **THEN** the body is truncated to 5 characters (`"café_"` or `"日本語测试"`) without panicking
- **AND** the truncated string is valid UTF-8

#### Scenario: Truncate on pure ASCII is unchanged

- **GIVEN** a LogProducer with `maxChars=10`
- **WHEN** the body string is `"hello world"` (11 chars)
- **THEN** the body is truncated to `"hello worl"` (10 chars)

#### Scenario: Existing test updated to char semantics

- **GIVEN** the existing `test_log_config_max_chars_param` test
- **WHEN** `max_chars` is set to 10
- **THEN** the assertion checks `.chars().count() <= 10`, not `.len() <= 10`

### Requirement: SEDA concurrent consumer delivery

The SEDA component SHALL spawn one forwarder task per `concurrentConsumers`
value when the mode is Single, using a shared `tokio::sync::Mutex<Receiver>`
so that forwarders process envelopes in parallel.

#### Scenario: concurrentConsumers=4 spawns four forwarders

- **GIVEN** a SEDA endpoint configured with `concurrentConsumers=4`
- **WHEN** the consumer is started
- **THEN** four forwarder tasks are spawned
- **AND** four JoinHandles are stored in `forwarder_handles`
- **AND** `concurrency_model()` reports `Concurrent { max: Some(4) }`

#### Scenario: InOut exchanges process in parallel

- **GIVEN** a SEDA endpoint with `concurrentConsumers=2` and an InOut consumer that sleeps 100ms per envelope
- **WHEN** two envelopes are enqueued simultaneously
- **THEN** both envelopes complete within 200ms (parallel), not 400ms (serial)

#### Scenario: concurrentConsumers=1 preserves single-forwarder behavior

- **GIVEN** a SEDA endpoint configured with `concurrentConsumers=1`
- **WHEN** the consumer is started
- **THEN** exactly one forwarder task is spawned

#### Scenario: Lock is not held during processing

- **GIVEN** a SEDA endpoint with `concurrentConsumers=2`
- **WHEN** two envelopes are enqueued and the consumer blocks on the first
- **THEN** the second forwarder acquires the receiver lock and processes the second envelope concurrently

### Requirement: Unique proto descriptor across concurrent processes

The proto compiler SHALL produce descriptor-set files with paths that are
unique across OS processes, preventing concurrent-build clobbering.

#### Scenario: Two concurrent compilations do not clobber

- **GIVEN** two OS processes that each call `compile_proto` at the same time
- **WHEN** both write their descriptor output
- **THEN** each process writes to a distinct temp file path
- **AND** neither process reads the other's output

#### Scenario: Descriptor file is cleaned up after use

- **GIVEN** a `compile_proto` invocation that creates a NamedTempFile
- **WHEN** the descriptor file handle is dropped
- **THEN** the temp file is removed by the OS (no orphan files in temp_dir)

### Requirement: Container cleanup respects configured docker_host

The container cleanup function SHALL connect to the Docker daemon using the
configured `docker_host` when one is provided, not silently fall back to
the default socket.

#### Scenario: Non-default socket cleanup uses configured host

- **GIVEN** a container component configured with `dockerHost=unix:///custom/docker.sock`
- **WHEN** `cleanup_tracked_containers(Some("unix:///custom/docker.sock"))` is called
- **THEN** the cleanup connects to `unix:///custom/docker.sock`
- **AND** tracked container IDs are removed from that daemon

#### Scenario: No docker_host falls back to defaults

- **GIVEN** a cleanup call with `docker_host=None`
- **WHEN** `cleanup_tracked_containers(None)` connects
- **THEN** it uses `Docker::connect_with_local_defaults()`

### Requirement: WSS readiness deferred until TLS listener bind

The WebSocket consumer SHALL NOT signal readiness before the TLS listener
is bound for `wss://` routes. It SHALL use `axum_server::Handle::listening()`
to detect bind success and propagate bind failure synchronously to
`start()`.

#### Scenario: wss bind failure does not signal ready

- **GIVEN** a `wss://` endpoint whose TLS listener bind fails (port conflict)
- **WHEN** `WsConsumer::start()` is called
- **THEN** `start()` returns an error
- **AND** `mark_ready()` is never called

#### Scenario: wss bind success signals ready after listening

- **GIVEN** a `wss://` endpoint whose TLS listener binds successfully
- **WHEN** `axum_server::Handle::listening()` resolves
- **THEN** `mark_ready()` is called after the listening signal
- **AND** `start()` returns `Ok(())`

#### Scenario: plain ws readiness is unchanged

- **GIVEN** a `ws://` endpoint
- **WHEN** `TcpListener::bind` succeeds synchronously
- **THEN** `mark_ready()` is called after the synchronous bind
- **AND** the behavior is identical to pre-change

### Requirement: BeanError non-exhaustive

The `BeanError` public enum SHALL carry `#[non_exhaustive]` so that adding
a variant post-1.0 is not a breaking change.

#### Scenario: External match requires wildcard arm

- **GIVEN** `BeanError` with `#[non_exhaustive]`
- **WHEN** an external consumer matches on `BeanError`
- **THEN** the compiler requires a `_ =>` arm
- **AND** all existing variant construction sites compile unchanged

#### Scenario: All existing tests pass

- **GIVEN** the camel-bean test suite (23 tests)
- **WHEN** `#[non_exhaustive]` is added
- **THEN** all 23 tests pass without modification

### Requirement: Endpoint macros trybuild regression suite expansion

The endpoint-macros crate SHALL expand its existing trybuild compile-fail
test suite to lock the error messages of the proc-macro's primary DX
contract paths that are not yet covered.

#### Scenario: Missing uri_scheme attribute

- **GIVEN** a struct deriving `UriConfig` without `#[uri_scheme]`
- **WHEN** trybuild compiles the ui case
- **THEN** compilation fails with the expected "missing uri_scheme" error message

#### Scenario: Non-struct input rejected

- **GIVEN** an enum deriving `UriConfig`
- **WHEN** trybuild compiles the ui case
- **THEN** compilation fails with the "UriConfig can only be derived for structs" message

#### Scenario: Duplicate path field rejected

- **GIVEN** a struct with two fields flagged as the path field
- **WHEN** trybuild compiles the ui case
- **THEN** compilation fails with the "only one field can be the path field" message

#### Scenario: New cases are discovered by existing harness

- **GIVEN** the new `*_fail.rs` files in `tests/ui/`
- **WHEN** `cargo test -p camel-endpoint-macros` is executed
- **THEN** the existing `ui_tests.rs` harness auto-discovers them via `tests/ui/*_fail.rs`
- **AND** all ui cases pass (expected compile failures match their .stderr)
