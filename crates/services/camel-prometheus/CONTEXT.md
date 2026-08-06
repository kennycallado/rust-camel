# camel-prometheus

Prometheus implementation of `MetricsCollector`. It exposes collected metrics and
the shared health routes through an Axum server managed as a `Lifecycle` service.

## Language

**PrometheusMetrics**:
`MetricsCollector` implementation that owns the Prometheus registry. It registers
fixed Camel metrics and creates dynamic counters and histograms on first use.
_Avoid_: Prometheus service, metrics server

**PrometheusService**:
`Lifecycle` implementation that owns `PrometheusMetrics` and starts or stops the
HTTP server. It also supplies the metrics instance as the Runtime's
`MetricsCollector`.
_Avoid_: Prometheus registry, scrape endpoint

**MetricsServer**:
Axum server that exposes `/metrics` and merges the `/healthz`, `/readyz`,
`/startupz`, and `/health` routes from `camel-health`.
_Avoid_: data-plane HTTP server, authenticated API

**DefaultHealthSource**:
Fallback health source used when no Runtime `HealthSource` is set. It reports all
health states as `Healthy`.
_Avoid_: Runtime health source, readiness registry

## Metrics exposure posture

The diagnostic HTTP routes are unauthenticated by Prometheus convention. This crate
does not provide TLS. Operators must restrict a non-loopback listener with network
policy or a firewall. The configuration path is opt-in through
`observability.prometheus.enabled`, but `PrometheusService::new` starts from the
address supplied by its caller and has no equivalent enablement guard.

ADR-0052 settles the shared diagnostic-endpoint posture: unauthenticated by scrape
convention, TLS and auth as opt-in hooks, loopback-preferred bind with a warning on
non-loopback opt-out. bd `rc-asm9` tracks the code work (bind default, warning, hooks).

## Cardinality contract

Dynamic label values must come from a closed or bounded set. Never pass raw Exchange
body, header, property, or correlation-key data as a label value. Each distinct label
combination creates a Prometheus series. The dynamic registries have no cardinality
cap or eviction, so unbounded values can cause unbounded memory growth. bd `rc-0pyv`
tracks this risk. ADR-0032 supplies the exchange-data trust boundary; its amendment
(2026-08-06) names metric label values as an unbounded resource sink and requires
closed-set or bounded label values here.

Counter observations reject NaN, negative, and fractional values. Histogram
observations reject NaN. These value checks do not bound label cardinality.

## Lifecycle status limitation

`PrometheusService::start` marks the service as `Started` after it spawns the server
task. If that task later exits with an error, it logs a warning but does not change
the stored status to `Failed`. `Lifecycle::status()` can therefore report `Started`
for a dead server. bd `rc-7zr3` tracks this status-fidelity bug.

## `#[non_exhaustive]` posture

ADR-0049 does not place this service crate in its mandatory contract-crate set.
The crate defines no public enums, so no case-by-case enum decision is required.

## Authority

- ADR-0032: exchange-data trust boundary and bounded resource decisions
- ADR-0049: workspace `#[non_exhaustive]` policy; not applicable to this crate
- ADR-0052: diagnostic-endpoint exposure posture
