# camel-bridge

Cross-language bridge service that spawns and manages subprocesses (JMS, XML, CXF bridges).
Communication uses mutual TLS (mTLS) with ephemeral rcgen-generated certificates.

## ADR-0012 log-policy annotations

| File | Line | Category | Reason |
|------|------|----------|--------|
| `src/process.rs` | 314 | `degraded` | Bounded stdout drain — oversized line truncated |
| `src/process.rs` | 324 | `normal` | Bounded stdout drain — child stdout line (debug) |
| `src/process.rs` | 351 | `normal` | Bounded stdout drain — drop summary in rate-limit interval |
| `src/process.rs` | 496 | `system-broken` | Subprocess startup — bridge ready message malformed |
| `src/process.rs` | 503 | `system-broken` | Subprocess startup — stdout closed before ready message |
| `src/process.rs` | 513 | `system-broken` | Subprocess startup — health check timeout |
