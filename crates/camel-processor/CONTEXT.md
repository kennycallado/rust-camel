# Processor

EIP (Enterprise Integration Pattern) processors implemented as Tower middleware services. Each
processor compiles from a DSL Step and is composed into the route pipeline.

## ADR-0012 log-policy sites

All 5 sites in this crate are category **(a) handler-owned** — EIP processor failures that occur
INSIDE the pipeline where the route ErrorHandler owns ERROR responsibility. These have been
downgraded from `error!` to `warn!`.

| File | Line | Category | Annotation |
|------|------|----------|------------|
| `aggregator.rs` | 119 | (a) handler-owned | `// log-policy: handler-owned` — force_complete_all failed |
| `aggregator.rs` | 463 | (a) handler-owned | `// log-policy: handler-owned` — timeout task failed |
| `log.rs` | 64 | (a) handler-owned | `// log-policy: handler-owned` — LogProcessor::call default Error level |
| `log.rs` | 113 | (a) handler-owned | `// log-policy: handler-owned` — DynamicLog::call default Error level |
| `wire_tap.rs` | 87 | (a) handler-owned | `// log-policy: handler-owned` — WireTap processing error |

## Metrics

Metrics instrumentation is not yet wired for most processors. See TODO(PROC-004) in
`camel-processor/src/log.rs` for the broader instrumentation gap.
