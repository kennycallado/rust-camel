## ADDED Requirements

### Requirement: Prometheus name normalization is idempotent

The Prometheus exporter SHALL apply the `camel_` prefix to a dynamic metric
name at most once. A name already carrying the `camel.` (dot) or `camel_`
(underscore) prefix MUST normalize to a single `camel_` prefix after
sanitization; a foreign name MUST receive exactly one `camel_` prefix.

#### Scenario: dotted camel.* built-in normalizes to a single prefix

- **GIVEN** a recorded built-in metric name in dot form
  (`camel.cache.misses`)
- **WHEN** the name is normalized for Prometheus export
- **THEN** the exported name is `camel_cache_misses` — the `camel_` prefix
  appears exactly once

#### Scenario: underscore camel_ name is unchanged

- **GIVEN** a metric name already carrying the underscore prefix
  (`camel_exchanges_total`)
- **WHEN** the name is normalized for Prometheus export
- **THEN** the exported name is `camel_exchanges_total`, unchanged

#### Scenario: foreign name is prefixed exactly once

- **GIVEN** a foreign metric name with no camel prefix
  (`exec_successes_total`)
- **WHEN** the name is normalized for Prometheus export
- **THEN** the exported name is `camel_exec_successes_total`
