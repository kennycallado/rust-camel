# camel-component-log

The Log component routes Exchange bodies and headers to `tracing` at a
user-selected level. It is producer-only. `LogEndpoint::create_consumer`
returns an error, while `LogEndpoint::create_producer` creates a pass-through
`LogProducer`.

See `crates/components/CONTEXT.md` for the shared Component, Endpoint, and
Producer vocabulary.

## Language

**LogComponent**:
Component registered under the `log` scheme. It parses the URI and creates a
LogEndpoint.
_Avoid_: logger, logging adapter

**LogEndpoint**:
Crate-private Endpoint that holds a parsed LogConfig and creates a LogProducer.
It does not support Consumers.
_Avoid_: log URI, log destination

**LogProducer**:
Crate-private `Service<Exchange>` that formats and emits configured Exchange
content, then returns the Exchange unchanged.
_Avoid_: tracing subscriber, log sink

**LogConfig**:
URI-derived configuration for category, level, body and header display,
truncation, masking, stream information, and group logging.
_Avoid_: tracing configuration

**LogLevel**:
Closed set of supported output levels: Trace, Debug, Info, Warn, and Error.
_Avoid_: tracing level filter

## ADR-0012 exclusion

`LogProducer` is a user-output mechanism. A route selects its output level, so
the `error!` invocation in `impl Service<Exchange> for LogProducer` is outside
ADR-0012's operational error-level convention. The `lint-log-levels` exclusion
is symbol-bound to that impl, not file-bound. The contract is guarded by
`tests/logproducer_exclusion_regression.rs`. Other `error!` sites in this crate
remain subject to ADR-0012.

## Public enum posture

ADR-0049 does not apply to this crate. It binds public contract enums in
`camel-api`, `camel-component-api`, and `camel-language-api`. LogLevel remains
exhaustive because its five variants are the component's closed set of output
levels.

## Security posture

ADR-0051 does not apply to LogConfig because it stores URI metadata, not
credential bytes. The `logMask=true` option is an output-redaction feature. It
replaces the complete body and values of headers whose names match the
sensitive-name heuristic. This is defense in depth, not credential storage.

Masking is disabled by default. With `logMask=false`, configured body and header
output can disclose sensitive Exchange data. Routes that handle such data must
enable masking or disable the corresponding output.

## Related decisions

- ADR-0012: operational log-level convention. LogProducer has the narrow
  symbol-bound exclusion described above.
- ADR-0049: workspace `#[non_exhaustive]` policy. This crate is outside scope.
- ADR-0051: credential redaction at diagnostic boundaries. LogConfig holds no
  credential bytes.
