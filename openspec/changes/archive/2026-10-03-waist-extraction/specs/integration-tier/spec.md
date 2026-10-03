## ADDED Requirements

### Requirement: Uniform datasource steering across state families

Every state-family operation — a prepare action and a validate target,
in each state family — SHALL resolve its datasource by name through the
boot's datasource catalog in one uniform way. A resolution failure SHALL
name the operation's family label (`sql action`, `sql validation`,
`surreal action`, `surreal validation`) and the datasource name, and
SHALL NOT contain the datasource URL: every exact, nonempty occurrence
of the `db_url` SHALL be replaced by `[REDACTED]` before the failure is
reported, while existing driver detail SHALL be retained otherwise.

#### Scenario: unknown datasource in a prepare action names the label and the name

- **GIVEN** a `sql:` prepare action naming datasource `appdb` and a boot
  whose catalog has no `appdb`
- **WHEN** the action executes
- **THEN** the failure is apparatus-class and its message reads
  `sql action: unknown datasource 'appdb'`, with the same shape
  (`surreal action: ...`) for a `surreal:` prepare action

#### Scenario: unknown datasource in a validate target names the label and the name

- **GIVEN** a `validate` sql target naming datasource `appdb` and a boot
  whose catalog has no `appdb`
- **WHEN** the action executes
- **THEN** the failure is apparatus-class and its message reads
  `sql validation: unknown datasource 'appdb'`, with the same shape
  (`surreal validation: ...`) for a surreal target

#### Scenario: a pool failure redacts the datasource URL

- **GIVEN** a state-family operation whose datasource URL carries a
  secret path component
- **WHEN** pool creation fails
- **THEN** the failure message names the label and the datasource, and
  every occurrence of the `db_url` reads `[REDACTED]`

#### Scenario: a handle downcast failure keeps its driver detail

- **GIVEN** a state-family operation whose resolved pool handle is not
  the handle type the family expects
- **WHEN** the handle is downcast
- **THEN** the failure message names the label and the datasource,
  retains the downcast driver detail, and contains no datasource URL

### Requirement: Deadline poll driver contract

Every validate target that polls under a deadline — partner, sql, and
surreal — SHALL run one shared poll discipline: without a deadline, one
immediate snapshot SHALL decide; with a deadline, the harness SHALL fix
the expiry instant before the first snapshot and poll snapshots until
it, where a snapshot error SHALL stop the poll at once, a
family-supplied early judgment MAY stop the poll before the expiry
instant (an in-flight snapshot is never cancelled by the deadline; its
early judgment still precedes the expiry decision), the snapshot at
expiry SHALL decide otherwise, and the sleep between snapshots SHALL
never exceed the remaining window. Per-family poll semantics (which
judgments settle early, which wait the window) SHALL stay owned by the
family requirements; this contract governs only the shared discipline.

#### Scenario: no deadline decides on one immediate snapshot

- **GIVEN** a `validate` action with a count bound and no `deadline`
- **WHEN** the action executes against a partner whose recorder already
  holds a satisfying count
- **THEN** the action decides on that single snapshot without polling

#### Scenario: an early judgment stops the poll before the deadline

- **GIVEN** a partner `validate` action with `deadline: 5s` and an
  `atLeast` count bound
- **WHEN** the filtered count reaches the bound while retries are still
  in flight
- **THEN** the action passes without waiting the full deadline

#### Scenario: the snapshot at expiry decides an absence claim

- **GIVEN** a partner `validate` action with `deadline: 2s` and an
  `atMost` bound the mid-window snapshots satisfy
- **WHEN** the deadline elapses
- **THEN** the action decides on the final snapshot, not on an earlier
  passing one

#### Scenario: a snapshot error stops the poll at once

- **GIVEN** a sql `validate` action with a deadline whose datasource
  query fails on the first snapshot
- **WHEN** the poll takes its first snapshot
- **THEN** the action fails apparatus-class immediately, without
  further snapshots or sleeps
