# camel-bridge

Cross-language bridge service that spawns and manages subprocesses (JMS, XML, CXF bridges).

## ADR-0012 log-policy annotations

| File | Line | Category | Reason |
|------|------|----------|--------|
| `src/process.rs` | 366 | `system-broken` | Subprocess startup — bridge ready message malformed |
| `src/process.rs` | 371 | `system-broken` | Subprocess startup — stdout closed before ready message |
| `src/process.rs` | 381 | `system-broken` | Subprocess startup — health check timeout |
