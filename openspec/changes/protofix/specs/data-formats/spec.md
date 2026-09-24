## ADDED Requirements

### Requirement: Protobuf runtime protoc resolution

The protobuf data format SHALL resolve the `protoc` binary at runtime in
this order: the `PROTOC` environment variable first, the vendored
protoc fallback second. When `PROTOC` is set, the system SHALL NOT
execute the vendored lookup; an explicit override is honored verbatim,
so a `PROTOC` value that fails at execution time SHALL surface the
execution error and SHALL NOT fall back to the vendored binary. When
`PROTOC` is unset and the vendored lookup fails, either by panicking
(missing baked binary) or by returning an error (unsupported platform),
the proto compilation SHALL fail with a typed
`ProtoCompileError::ProtocUnavailable` whose message names the `PROTOC`
remedy, and the process SHALL stay alive; route and document load
surface the error as a normal load failure. When `PROTOC` is unset and
the vendored binary is present, compilation SHALL behave exactly as
before this requirement.

#### Scenario: PROTOC override serves the compilation end to end

- **GIVEN** `PROTOC` set to an executable script that records its
  invocation in a marker file and serves a valid descriptor set for the
  `--descriptor_set_out` argument
- **WHEN** `compile_proto` runs on a `.proto` file
- **THEN** compilation succeeds through the script, the marker file
  exists, and the returned descriptor pool exposes the expected message

#### Scenario: The vendored lookup never runs while PROTOC is set

- **GIVEN** `PROTOC` set and a resolution seam injected with a vendored
  resolver that signals any invocation (by panicking or by recording)
- **WHEN** protoc resolution runs
- **THEN** the resolver is not invoked and the environment path is
  returned

#### Scenario: A broken PROTOC override fails without vendored fallback

- **GIVEN** `PROTOC` set to a path that cannot execute (missing file)
- **WHEN** `compile_proto` runs on a `.proto` file
- **THEN** the failure is the ordinary execution error and the vendored
  lookup is not attempted

#### Scenario: Missing vendored protoc fails with a typed error and the process stays alive

- **GIVEN** `PROTOC` unset and a vendored resolver that panics with the
  vendored crate's "internal: protoc not found" shape (simulated
  absence of the vendored binary)
- **WHEN** protoc resolution runs
- **THEN** the result is a typed `ProtoCompileError::ProtocUnavailable`
  carrying the panic detail, and the calling test completes (the panic
  is contained, not propagated to the process)

#### Scenario: Vendored fallback unchanged when PROTOC is unset and present

- **GIVEN** `PROTOC` unset and the vendored protoc binary present (CI
  shape)
- **WHEN** `compile_proto` runs on the existing test fixtures
- **THEN** compilation succeeds through the vendored binary exactly as
  before this change (regression)
