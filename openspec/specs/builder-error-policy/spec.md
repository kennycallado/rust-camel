# builder-error-policy Specification

## Purpose
TBD - created by archiving change unify-builder-error-policy. Update Purpose after archive.
## Requirements
### Requirement: Builder fluent API SHALL NOT panic on user-reachable misuse

No public method on the `camel-builder` fluent API (`RouteBuilder`, child builders
including `DoTryBuilder`, `DoCatchBuilder`, `DoFinallyBuilder`) SHALL panic on input or
state reachable through normal use of the API. Misuse SHALL be either prevented at the
type level (the offending input is not expressible) or reported via
`Result<_, CamelError>`. This policy generalizes the workspace panic-vs-`Result`
invariant for the T1 sweep.

#### Scenario: no panic macro remains in the library target

- **GIVEN** the `camel-builder` library target (excluding `#[cfg(test)]` modules)
- **WHEN** `cargo clippy -p camel-builder --lib -- -D clippy::panic` runs
- **THEN** the command exits successfully (no `panic!` macro in the library source)

#### Scenario: Continued disposition is not expressible

- **GIVEN** a `DoCatchBuilder` obtained from `.do_catch_exception(...)`,
  `.do_catch_when(...)`, or `.do_catch_all()`
- **WHEN** a `compile_fail` doctest on the `handled()` / `propagate()` sugar methods
  references `.disposition(ExceptionDisposition::Continued)`
- **THEN** the doctest fails to compile, mechanically proving no public method accepts
  `ExceptionDisposition::Continued`; only `handled()` and `propagate()` sugar exist

### Requirement: doTry finally double-call SHALL return a RouteError

`DoTryBuilder::do_finally` SHALL return `Result<DoFinallyBuilder, CamelError>`. A second
call within the same `doTry` scope (i.e. after `end_do_finally()` has handed back a
`DoTryBuilder` with `finally_set == true`) SHALL return
`Err(CamelError::RouteError(_))` with a message that names the misuse, instead of
panicking.

#### Scenario: second do_finally call returns Err

- **GIVEN** a `RouteBuilder` with an open `doTry` scope that already has a `doFinally`
  block closed via `end_do_finally()`
- **WHEN** `do_finally()` is called again on the returned `DoTryBuilder`
- **THEN** the result is `Err(CamelError::RouteError(msg))` where `msg` states that
  `do_finally` can only be called once per `do_try` scope

#### Scenario: first do_finally call succeeds

- **GIVEN** a `RouteBuilder` with an open `doTry` scope and no `doFinally` block yet
- **WHEN** `do_finally()` is called
- **THEN** the result is `Ok(DoFinallyBuilder)` and the returned builder accepts
  `.process(...)` / `.on_when(...)` / `.end_do_finally()` as before

### Requirement: doTry catch disposition sugar methods set the field directly

`DoCatchBuilder::handled()` and `DoCatchBuilder::propagate()` SHALL set the catch
clause's disposition by assigning the private `disposition` field directly to
`ExceptionDisposition::Handled` or `ExceptionDisposition::Propagate` respectively. The
general-purpose `disposition(value: ExceptionDisposition)` method SHALL NOT exist on the
public API.

#### Scenario: handled sugar sets Handled disposition

- **GIVEN** a `DoCatchBuilder` (default disposition `Handled`)
- **WHEN** `.handled()` is called on it
- **THEN** the builder's catch clause carries `ExceptionDisposition::Handled` after
  `end_do_catch()` closes the clause

#### Scenario: propagate sugar sets Propagate disposition

- **GIVEN** a `DoCatchBuilder`
- **WHEN** `.propagate()` is called on it
- **THEN** the builder's catch clause carries `ExceptionDisposition::Propagate` after
  `end_do_catch()` closes the clause

