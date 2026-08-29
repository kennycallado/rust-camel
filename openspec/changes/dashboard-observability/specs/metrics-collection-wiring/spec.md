## MODIFIED Requirements

### Requirement: Tracer pipeline implied by prometheus enablement

The system SHALL enable the tracer pipeline when
`[observability.prometheus]` is enabled, so that the non-disableable
error family is always exported; the new `[observability.metrics]`
levers (metrics-configuration capability) may suppress individual
non-error families but SHALL NOT disable the pipeline itself.

#### Scenario: prometheus-only still runs the pipeline with metrics opt-outs

- **GIVEN** no metrics family opt-outs are set
- **WHEN** only `[observability.prometheus]` is enabled
- **THEN** the tracer pipeline is enabled and component metrics reach
  `GET /metrics`
- **AND** `camel_errors_total` is exported regardless of any
  `[observability.metrics]` lever combination
