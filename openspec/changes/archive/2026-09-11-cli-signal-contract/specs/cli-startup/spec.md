## ADDED Requirements

### Requirement: camel run force-exits on repeated stop signals

The `camel run` command SHALL treat the first SIGINT or SIGTERM as a graceful
shutdown at any time, including during boot, and SHALL force-exit with code 1
when a subsequent SIGINT or SIGTERM arrives after the first was consumed and
graceful shutdown has begun. The signal streams SHALL be armed before boot
starts so an early signal is buffered, not default-killed. Identical signal
bursts may coalesce before delivery, so strict signal counting is not
guaranteed. On platforms without SIGTERM, the portable Ctrl+C listener is
the first-signal handler and a second Ctrl+C force-exits.

#### Scenario: stop signal during boot shuts down gracefully

- **GIVEN** a `camel run` process that is still booting (config load,
  component boot, route discovery, or context start in flight)
- **WHEN** the operator sends SIGTERM or SIGINT
- **THEN** boot completes, the shutdown select consumes the buffered signal,
  and the process exits with code 0

#### Scenario: second stop signal during teardown force-exits

- **GIVEN** the first stop signal was consumed and graceful teardown is
  running
- **WHEN** a second SIGTERM or SIGINT arrives before teardown completes
- **THEN** the process logs `Second ... — forcing exit` and exits with
  code 1

#### Scenario: single stop signal during normal running stays graceful

- **GIVEN** a `camel run` process whose context has started
- **WHEN** the operator sends exactly one SIGTERM or SIGINT
- **THEN** the process shuts down gracefully and exits with code 0; the
  force-exit arm never fires
