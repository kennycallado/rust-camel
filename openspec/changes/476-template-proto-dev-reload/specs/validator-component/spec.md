## ADDED Requirements

### Requirement: Protobuf descriptor hot reload

A validator endpoint with a protobuf schema SHALL register a reload target in
the `TemplateReloadRegistry` for its route when a route id is available (the
`validator-bridge` fallback id registers nothing), holding the compiled
descriptor behind an atomic swap cell and exposing the committed schema file
plus its resolved local imports as watch sources. On reload, the endpoint
SHALL recompile the `.proto` in-process within a fixed 5000 ms deadline
matching camel-template's `reload_timeout_ms` — with the synchronous
compilation isolated from the async runtime — and swap the descriptor
together with the recomputed source list atomically. A failed or timed-out
recompile SHALL keep the previously committed descriptor and leave the
endpoint serving, and no commit SHALL happen after the deadline passes.

#### Scenario: descriptor recompiles on reload

- **GIVEN** a running validator endpoint with a compiled protobuf descriptor
- **WHEN** the `.proto` source changes and the route's reload runs
- **THEN** the descriptor is recompiled and swapped, and the next validation
  uses the new schema

#### Scenario: import-only edit recompiles the descriptor

- **GIVEN** a running validator endpoint whose schema imports another local
  `.proto` file
- **WHEN** only the imported file changes and the route's reload runs
- **THEN** the recompiled descriptor reflects the import change after the
  swap

#### Scenario: reload that adds a local import reports the new source

- **GIVEN** a running validator endpoint with a committed source list
- **WHEN** a reload commits a schema that imports a new local `.proto` file
- **THEN** the new file appears in the target's watch sources afterwards

#### Scenario: failed recompile keeps prior descriptor

- **GIVEN** a running validator endpoint with a compiled protobuf descriptor
- **WHEN** the `.proto` source becomes invalid and the route's reload runs
- **THEN** the reload fails, the prior descriptor keeps validating, and the
  endpoint stays in service

#### Scenario: stalled build times out without commit

- **GIVEN** a validator reload whose compilation does not finish within the
  configured deadline
- **WHEN** the deadline passes
- **THEN** the reload is abandoned with an error and no descriptor swap
  happens afterwards
