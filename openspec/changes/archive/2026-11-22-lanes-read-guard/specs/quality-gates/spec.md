## ADDED Requirements

### Requirement: Lane table read and decode failures are a named exit 2

The component-lane phase of `scripts/gates-local.sh` SHALL treat a failure to
read or UTF-8 decode `AGENTS.md` as a fail-closed error: it SHALL print a line
beginning `gates-local: lanes: cannot read <path>:` followed by the underlying
error to stderr, exit with status 2, print no Python traceback, and dispatch no
lane command. The message and exit status SHALL match the registry grammar's
read guard except for the `lanes` label. A table that reads and decodes
successfully SHALL parse exactly as before.

#### Scenario: Lane table that cannot be decoded is a named exit 2

- **GIVEN** a registry gate that rewrites `AGENTS.md` with invalid UTF-8 after
  the registry phase has parsed it, and the lane phase is selected
- **WHEN** the lane phase of `scripts/gates-local.sh` parses `AGENTS.md`
- **THEN** stderr contains `gates-local: lanes: cannot read`, stderr contains
  no `Traceback`, the sweep exits 2, and no lane command is dispatched
