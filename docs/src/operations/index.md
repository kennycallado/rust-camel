# Operations

Production concerns: health endpoints, graceful shutdown, and route lifecycle.
The runtime exposes HTTP probes for orchestrators and drains in-flight exchanges
on shutdown.

- [Health](health.md): liveness and readiness probes, Degraded and Unhealthy states, `ObservabilityConfig.health` wiring.
- [health-demo](../../../examples/health-demo/): runnable server with `/healthz`, `/readyz`, and `/health` endpoints.

Graceful shutdown drains in-flight exchanges up to `drain_timeout_ms` during
`CamelContext::stop`. Route lifecycle control via ControlBus (ADR-0034) does
not yet have a narrative page.
