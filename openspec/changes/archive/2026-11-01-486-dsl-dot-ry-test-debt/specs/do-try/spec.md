## MODIFIED Requirements

### Requirement: A Handled or Continued catch exits with the error cleared

The doTry block SHALL run the catch body with the try error visible on the
exchange, so that exchange-property lookups such as `${exception}` and
`${exception.message}` resolve inside the catch body. When a `Handled` or
`Continued` catch body completes, the block SHALL clear the exchange error
before running an enabled finally body and before returning the exchange.

A catch clause whose disposition is `Handled` SHALL clear the error through
`Exchange::handle_error()`, which also sets the `CamelExceptionHandled`
exchange property to `true`. A catch clause whose disposition is `Continued`
SHALL clear the error through `Exchange::clear_error()` and SHALL NOT set the
`CamelExceptionHandled` property. Clearing SHALL follow the existing
`Exchange::clear_error()` contract, which removes the error together with the
exception properties that `Exchange::set_error()` derived from it, including
`CamelExceptionCaught`. Steps after a `Handled` or `Continued` exit SHALL NOT
rely on `CamelExceptionCaught`; a step that needs the caught exception SHALL
read or copy it inside the catch body.

`CamelExceptionHandled` is a Camel-compatibility marker only; control flow
SHALL key off `PipelineOutcome`, never off the marker. `DoTrySegment` SHALL
remove a stale marker on entry, and `clear_error()` SHALL NOT remove the
marker. Therefore a nested `Handled` block inside an outer `Continued` catch
body survives that outer catch's error clearing. A marker set by a nested
block inside a try body that later fails SHALL NOT reach the catch chain: the
failed outcome carries no exchange, and the catch chain starts from the
pre-try snapshot.

A `Propagate` catch SHALL return `Failed` with the try error, as before. A
catch body that fails, a failure that no catch clause matches, and a Stop
raised in the try body or a catch body SHALL keep their existing semantics:
the block does not clear the error on those paths. The existing requirements
for finally-run count and catch-body failure cover those exits.

#### Scenario: Handled catch clears the error and marks it handled

- **GIVEN** a doTry block whose try body fails, with a matching catch clause
  whose disposition is `Handled`
- **WHEN** the catch body observes the exchange and then completes
- **THEN** the catch body sees the try error on the exchange
- **AND** the block returns `Completed` with an exchange for which
  `has_error()` is false
- **AND** the `CamelExceptionHandled` property on that exchange is `true`

#### Scenario: Continued catch clears the error without the handled marker

- **GIVEN** a doTry block whose try body fails, with a matching catch clause
  whose disposition is `Continued`, and no nested doTry block sets the marker
  during that invocation
- **WHEN** the catch body observes the exchange and then completes
- **THEN** the catch body sees the try error on the exchange
- **AND** the block returns `Completed` with an exchange for which
  `has_error()` is false
- **AND** the `CamelExceptionHandled` property is absent from that exchange

#### Scenario: Nested Handled marker survives an outer Continued

- **GIVEN** an outer doTry block whose try body fails with an error matched by
  a catch clause whose disposition is `Continued`, and whose catch body
  contains a nested doTry block whose matching catch clause has the `Handled`
  disposition
- **WHEN** the outer catch body runs, the nested try body fails, its `Handled`
  catch body completes, and the outer catch body then completes
- **THEN** the nested block returns `Completed` and sets the
  `CamelExceptionHandled` property to `true`
- **AND** the outer block returns `Completed` and does not remove that marker
- **AND** the exchange after the outer block has `has_error()` false and
  carries `CamelExceptionHandled` still `true`

#### Scenario: A marker set in a failing try body does not reach the catch chain

- **GIVEN** an outer doTry block whose try body sets `CamelExceptionHandled`
  (for example through a nested `Handled` doTry block) and then fails
  independently, and whose catch clause has the `Continued` disposition
- **WHEN** the try body sets the marker and then fails
- **THEN** the catch chain starts from the pre-try exchange snapshot, which
  carries no marker
- **AND** the outer block returns `Completed` with the marker absent

#### Scenario: Steps after a Handled or Continued exit cannot rely on the caught property

- **GIVEN** a doTry block whose try body fails, with a matching catch clause
  whose disposition is `Handled` or `Continued`, and a step immediately after
  the block that reads `CamelExceptionCaught`
- **WHEN** the catch body completes and the block exits
- **THEN** the catch body saw the caught exception on the exchange
- **AND** the exchange returned by the block has no `CamelExceptionCaught`
  property, because clearing removed it with the error
- **AND** the step after the block does not observe the caught exception and
  SHALL NOT depend on it

#### Scenario: Finally after a handled catch observes a clean exchange

- **GIVEN** a doTry block with a matching `Handled` catch clause and an enabled
  finally body
- **WHEN** the catch body completes
- **THEN** the finally body runs exactly once and observes an exchange for
  which `has_error()` is false and `CamelExceptionMessage` is absent
- **AND** the block returns `Completed` with the finally output exchange

#### Scenario: Propagate catch keeps the error

- **GIVEN** a doTry block whose try body fails, with a matching catch clause
  whose disposition is `Propagate`
- **WHEN** the catch body completes
- **THEN** the outcome is `Failed` with the try error
- **AND** the error is not cleared by the block

#### Scenario: Handled catch under a route Unit of Work completes

- **GIVEN** a route with `on_complete` and `on_failure` hooks whose `doTry`
  block handles the try error
- **WHEN** the block completes
- **THEN** the `on_complete` hook runs once and the `on_failure` hook does not
  run
