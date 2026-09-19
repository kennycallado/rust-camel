# stream-component Specification Delta

## ADDED Requirements
### Requirement: stream:out and stream:err write exchange bodies as data

The `stream:out` and `stream:err` producers SHALL write the exchange
body to file descriptors 1 and 2 respectively as DATA: verbatim bytes,
no log level, no category, no log formatting, and no redaction
transform. The producer SHALL materialize bodies through
`Body::into_bytes(max_size)` (`String`/`&str`/`Vec<u8>` verbatim;
structured bodies through the body's byte materialization). With
`appendNewline=true` (the default) the producer SHALL append exactly one
`\n` after the body; with `appendNewline=false` it SHALL write the body
verbatim. `charset` SHALL accept only `utf-8` in v1 (any other value is
rejected at construction). The producer SHALL flush after every
exchange and SHALL serialize concurrent writes to the same descriptor.

#### Scenario: string body writes line-framed to stdout

- **GIVEN** a route with `to: stream:out` and an exchange whose body is
  the string `hello` (no trailing newline)
- **WHEN** the producer processes the exchange
- **THEN** fd 1 receives the bytes `hello\n` and the write is flushed
  before the producer completes

#### Scenario: appendNewline=false writes raw bytes

- **GIVEN** a route with `to: stream:out?appendNewline=false` and an
  exchange whose body is the bytes `a,b,c` (CSV fragment)
- **WHEN** the producer processes the exchange
- **THEN** fd 1 receives exactly `a,b,c` with no appended newline

#### Scenario: structured body materializes to bytes

- **GIVEN** a route with `to: stream:out` and an exchange whose body is
  a structured value (not `String`/`&str`/`Vec<u8>`)
- **WHEN** the producer processes the exchange
- **THEN** the body materializes to bytes through `Body::into_bytes`
  and the resulting bytes reach fd 1

#### Scenario: non-utf8 charset rejected at construction

- **GIVEN** a route declaring `to: stream:out?charset=latin-1`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::Config` naming utf-8 as
  the only v1 charset

#### Scenario: stream:err targets file descriptor 2

- **GIVEN** a route with `to: stream:err` and an exchange whose body is
  a prompt string
- **WHEN** the producer processes the exchange
- **THEN** fd 2 receives the body (with the same newline semantics) and
  fd 1 receives nothing

#### Scenario: credential-bearing body passes verbatim

- **GIVEN** a route with `to: stream:out` and an exchange body
  containing a credential-like string
- **WHEN** the producer processes the exchange
- **THEN** the body reaches fd 1 unmodified (no redaction is applied;
  the egress decision belongs to the route author and operator)

### Requirement: No redaction on the stream data plane

The stream producers SHALL NOT apply any redaction, masking, or log
formatting, and the log-redaction lint plane SHALL NOT be extended to
cover stdio data output. The un-redacted egress posture SHALL be
recorded as an accepted limitation in an ADR that distinguishes this
component from the ADR-0060 MCP stdio rejection (process supervision
versus the process's own file descriptors).

#### Scenario: redaction plane boundary is documented

- **GIVEN** the accepted-limitations ADR for the stream component
- **WHEN** the ADR is read alongside the lint-log-redaction allowlist
- **THEN** the ADR states that stdio data output is operator-owned
  trusted egress and the lint plane covers logging/tracing only

### Requirement: Tracer stdout collision warning

When a route uses a `stream:out` endpoint AND the observability tracer
stdout output is enabled at boot, the CLI boot path SHALL emit a `warn!`
naming the collision and the operator resolution (disable the tracer
stdout sink or route it elsewhere). The system SHALL NOT auto-mux,
auto-disable, or otherwise silently reconfigure either output.

#### Scenario: collision warns at boot

- **GIVEN** a route document using `to: stream:out` and a config with
  `[observability.tracer] enabled` and
  `[observability.tracer.outputs.stdout] enabled`
- **WHEN** the boot path loads the routes
- **THEN** a `warn!` describing the fd 1 collision is emitted and no
  output is reconfigured

#### Scenario: tracer disabled does not warn

- **GIVEN** a route document using `to: stream:out` and a config with
  the tracer stdout output disabled
- **WHEN** the boot path loads the routes
- **THEN** no collision warning is emitted

### Requirement: Slim default registration

The `stream` component SHALL be registered unconditionally in the
camel-bundles boot cascade (no feature gate) beside the lean set
(`timer`, `log`, `direct`, `seda`, `mock`), and the scheme SHALL resolve
after a slim boot.

#### Scenario: slim boot resolves the stream scheme

- **GIVEN** a context booted through the camel-bundles cascade with
  default (slim) features
- **WHEN** the component registry is queried for `stream`
- **THEN** the scheme resolves to the stream component

### Requirement: stream:in frames input as exchanges

The `stream:in` consumer SHALL emit one exchange per frame with
`frame=line` as the default (body is the line content without the
terminator), SHALL support `frame=raw` (the whole input as one exchange
at EOF; empty input emits zero exchanges) and `frame=fixed&size=N`
(N bytes per exchange; a final partial chunk emits one exchange; a
missing or zero `size` is rejected with `CamelError::InvalidUri` at
construction). `charset` SHALL accept only `utf-8` in v1 (any other
value is rejected at construction). Invalid UTF-8 in line mode SHALL
skip the line, record an error metric, and continue; raw and fixed
frames SHALL carry bytes verbatim.

#### Scenario: frame=fixed without a positive size is rejected

- **GIVEN** a route declaring `from: stream:in?frame=fixed` without
  `size`, or with `size=0`
- **WHEN** the endpoint is constructed
- **THEN** construction fails with `CamelError::InvalidUri`

#### Scenario: one line is one exchange

- **GIVEN** a started `from: stream:in` route and input containing three
  newline-terminated lines
- **WHEN** the consumer runs to EOF
- **THEN** exactly three exchanges enter the route, each with the line
  content (no terminator) as body

#### Scenario: final unterminated line still emits

- **GIVEN** a started `from: stream:in` route and input whose last line
  lacks a trailing newline
- **WHEN** the consumer reaches EOF
- **THEN** the final line emits as an exchange before completion

#### Scenario: raw frame emits one exchange at EOF

- **GIVEN** a started `from: stream:in?frame=raw` route and multi-line
  input
- **WHEN** the consumer reaches EOF
- **THEN** exactly one exchange enters the route with the entire input
  as body

#### Scenario: raw frame with empty input emits zero exchanges

- **GIVEN** a started `from: stream:in?frame=raw` route and stdin at
  EOF with no bytes
- **WHEN** the consumer runs
- **THEN** zero exchanges are emitted and the route completes gracefully

### Requirement: EOF completes the route gracefully

EOF on fd 0 SHALL complete the consumer normally: `start` returns
without error and the route completes. A stream:in route with no input
available (no TTY, no piped input) SHALL read EOF immediately, emit zero
exchanges, and complete. The consumer SHALL never use `isatty` or any
terminal detection for control flow; behavior SHALL key on input
presence only.

#### Scenario: no input completes with zero exchanges

- **GIVEN** a started `from: stream:in` route with stdin at EOF (no TTY,
  no piped input)
- **WHEN** the consumer runs
- **THEN** zero exchanges are emitted, no error is recorded, and the
  route completes gracefully

#### Scenario: cancellation stops the consumer cleanly

- **GIVEN** a started `from: stream:in` route with input pending
- **WHEN** the route's cancellation token fires
- **THEN** the consumer stops without error

### Requirement: stream:in respects sequential backpressure

The consumer SHALL NOT read the next frame until the current exchange's
send completes, and SHALL maintain no unbounded queue. A send failure
(channel closed) SHALL record an error metric and end the loop.

#### Scenario: slow route slows the read

- **GIVEN** a started `from: stream:in` route whose pipeline completes
  sends one at a time
- **WHEN** input arrives faster than the route processes it
- **THEN** the consumer reads at the route's pace (the kernel pipe
  buffer absorbs the slack; no queue grows without bound)

#### Scenario: closed channel ends the loop with an error metric

- **GIVEN** a started `from: stream:in` route
- **WHEN** the route channel closes while the consumer is sending
- **THEN** an error metric is recorded for the route and the consumer
  loop ends

### Requirement: camel job consumer allowlist admits exactly stream:in

The `camel job` load-time fail-closed consumer allowlist SHALL accept
the `stream` scheme only with path `in`. A job document naming
`from: stream:in` SHALL load; job documents naming `from: stream:out` or
`from: stream:err` SHALL fail closed at load with an error naming
`stream:in` as the only accepted stream consumer. All other non-allowed
schemes remain rejected.

#### Scenario: from stream:in loads

- **GIVEN** a job document whose route declares `from: stream:in`
- **WHEN** the document is loaded
- **THEN** the consumer gate passes

#### Scenario: from stream:out rejected at load

- **GIVEN** a job document whose route declares `from: stream:out`
- **WHEN** the document is loaded
- **THEN** loading fails with an error that names `stream:in` as the
  only accepted stream consumer path
