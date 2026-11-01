## ADDED Requirements

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
exception properties that `Exchange::set_error()` derived from it. The
retention of `CamelExceptionCaught` after a handled catch is a separate
decision and SHALL NOT be settled by this requirement.

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
  whose disposition is `Continued`
- **WHEN** the catch body observes the exchange and then completes
- **THEN** the catch body sees the try error on the exchange
- **AND** the block returns `Completed` with an exchange for which
  `has_error()` is false
- **AND** the `CamelExceptionHandled` property is absent from that exchange

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
