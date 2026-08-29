# metrics-collection-wiring

## ADDED Requirements

### Requirement: Error-path metric completeness

Every component failure site categorized (b′) or (e) by ADR-0012 SHALL
emit `MetricsCollector::increment_errors` with a label matching
`^(b-prime|e):[a-z][a-z0-9-]*:[a-z][a-z0-9-]+$`. A component SHALL NOT
store an observability handle that has neither eligible sites NOR live
observability/delegation use (audit OQ-5: exec and llm carry live
success-path metrics; master's runtime is live create_consumer
delegation; http-static's feeds get_or_spawn for the wired `e:http:*`
metrics — all four RETAINED).

#### Scenario: fresh audit enumerates the gaps

- **WHEN** the audit recipe (design D1) runs over all components
- **THEN** `audit.md` lists, per component, every eligible site as
  file:line + category ((b′)/(e) only), the wired subset, the gaps or a
  drop verdict, and — for drops — the associated stale comments and the
  public signatures preserved

#### Scenario: seda locally-terminal consumer failure is counted

- **GIVEN** a seda consumer whose forwarded `ctx.send` error is consumed
  locally (lib.rs:754-756 pattern) — i.e. the failure is NOT forwarded
  to a handler who can absorb it
- **WHEN** the failure is processed
- **THEN** `increment_errors` is called with a label
  `b-prime:seda:<site>` whose `<site>` matches
  `[a-z][a-z0-9-]*`

#### Scenario: drop verdict removes the dead field

- **GIVEN** a component whose stored observability handle has zero
  eligible sites per the audit
- **WHEN** the drop is applied
- **THEN** the field, its plumbing, and stale "Phase B" comments are gone
  and the crate still builds and passes its tests

#### Scenario: wired labels match the ADR-0012 regex

- **WHEN** any new `increment_errors` call from this change is inspected
- **THEN** its label matches `^(b-prime|e):[a-z][a-z0-9-]*:[a-z][a-z0-9-]+$`

#### Scenario: stale deferral comments are gone per drop row

- **GIVEN** a drop verdict enumerating a component's stale deferral
  comments ("Phase B will use this", "Phase-5", "deferred", "read later")
- **WHEN** the drop is applied
- **THEN** each enumerated comment is gone from that component
