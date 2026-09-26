# lifecycle-correctness delta — journstart2

## ADDED Requirements

### Requirement: Silent auto-startup suppression is observable

When the context start sequence completes, the system SHALL emit a WARN
naming the route for every auto-startup route that is registered but not in
the `Started` state at that point, including the case where the route's
`StartRoute` command was classified as a duplicate by command dedup and
suppressed. Routes whose definitions set `auto_startup` to false SHALL NOT
produce this warning, and genuine start failures (which fail the boot with
an error) remain errors rather than warnings.

#### Scenario: Duplicate-suppressed StartRoute warns

- **GIVEN** a registered route with `auto_startup` enabled whose `StartRoute`
  command ID is already recorded in the command dedup store
- **WHEN** the context start sequence runs and the `StartRoute` result is
  reported as a duplicate
- **THEN** a WARN naming the route (and the suppressed command ID) is
  emitted before the start sequence reports completion

#### Scenario: Registered but not started after the start loop warns

- **GIVEN** a registered route with `auto_startup` enabled that is not in
  the `Started` state after the start sequence's StartRoute loop completed
  without an error
- **WHEN** the start sequence reaches its post-loop check
- **THEN** a WARN naming the route is emitted

#### Scenario: Auto-startup-disabled routes stay silent

- **GIVEN** a registered route whose definition sets `auto_startup` to
  false
- **WHEN** the context start sequence completes without starting that route
- **THEN** no suppression warning is emitted for that route
