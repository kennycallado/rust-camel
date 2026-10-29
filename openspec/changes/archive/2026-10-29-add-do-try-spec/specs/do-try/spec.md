## ADDED Requirements

### Requirement: Finally runs exactly once per scope exit

The doTry block SHALL run an enabled finally body exactly once when the
scope exits by completion or by failure: try-body success, no catch
match, or a handled catch. A finally clause with an `on_when` predicate
is enabled only when the predicate returns true; a false predicate
SHALL skip the finally body entirely, and the pending outcome SHALL
stand unchanged. Only a Stop raised by the try body or a catch body
SHALL skip an otherwise-enabled finally body. The stopped exchange
SHALL propagate unchanged.

#### Scenario: Try body succeeds

- **GIVEN** a doTry block with a finally clause
- **WHEN** the try body completes
- **THEN** the finally body runs exactly once
- **AND** the outcome is Completed with the finally output exchange

#### Scenario: No catch matches the failure

- **GIVEN** a doTry block whose catch clauses do not match the try error
- **WHEN** the try body fails
- **THEN** the finally body runs exactly once
- **AND** the outcome is Failed with the original try error

#### Scenario: Catch clause handles the failure

- **GIVEN** a doTry block with a matching Handled catch clause
- **WHEN** the try body fails and the catch body completes
- **THEN** the finally body runs exactly once
- **AND** the outcome is Completed with the finally output exchange

### Requirement: Catch-body failure runs finally and surfaces the catch error

When a catch body fails, the doTry block SHALL run an enabled finally
body exactly once, then SHALL surface the catch error as the block
failure. Eligibility follows the scope-exit requirement. The block
SHALL NOT re-enter the catch chain. A failing finally body
SHALL keep the catch error as the surfaced failure, and the block SHALL
log a warning naming the restore. A failed finally `on_when` predicate
SHALL supersede the catch error with the predicate error. A Stop raised
in the finally body SHALL stop the outer route with the finally
exchange.

#### Scenario: Catch body fails

- **GIVEN** a doTry block with a matching catch clause whose body fails,
  and a finally clause without `on_when`
- **WHEN** the block runs
- **THEN** the finally body runs exactly once
- **AND** the outcome is Failed with the catch error, not the try error

#### Scenario: Finally body also fails

- **GIVEN** a doTry block whose catch body and finally body both fail,
  and a finally clause without `on_when`
- **WHEN** the block runs
- **THEN** the outcome is Failed with the catch error
- **AND** a warning records that the finally body threw and the
  previous error was restored, and the finally error does not surface

#### Scenario: Stop in finally after catch-body failure

- **GIVEN** a doTry block whose catch body fails and whose finally body
  stops, with no `on_when` on the finally clause
- **WHEN** the block runs
- **THEN** the outcome is Stopped with the finally exchange
