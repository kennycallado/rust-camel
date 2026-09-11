## ADDED Requirements

### Requirement: Master component exports leadership state gauge

The master component SHALL export the gauge `camel_master_is_leader{lock}`
through the configured `MetricsCollector` with value 1 while leadership is
held for `lock` and value 0 after leadership is lost. Emission SHALL occur
on the same observed leadership-state edges as
`master_leadership_transitions_total`, through the same code path (the
transition emission helper). The `lock` label value SHALL be the lock name,
identical to the leadership counters' `lock` label. The gauge provides
steady-state readability ("who leads lock X now"); the transition counters
retain their transition-only semantics and are not modified. The gauge
series appears on the first observed edge; no startup initialization is
required.

#### Scenario: gauge reads 1 while leadership is held

- **GIVEN** a master consumer whose leadership watch snapshot or delivery changes the observed state to leading for lock `my-lock`
- **WHEN** the acquire edge is emitted
- **THEN** the collector's `camel_master_is_leader` gauge for `lock="my-lock"` reads 1 for as long as leadership remains held

#### Scenario: gauge reads 0 after leadership is yielded

- **GIVEN** a master consumer that is leading for lock `my-lock`
- **WHEN** the leadership watch delivers a `StoppedLeading` event and the leading → not-leading edge is emitted
- **THEN** the collector's `camel_master_is_leader` gauge for `lock="my-lock"` reads 0

#### Scenario: gauge uses the uniform lock label

- **GIVEN** a master consumer configured with lock name `my-lock`
- **WHEN** any leadership-state edge is emitted
- **THEN** the gauge series carries exactly the label `lock="my-lock"`, the same label schema the `master_leadership_transitions_total` counter uses
