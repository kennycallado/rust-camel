# documentation Specification

## Purpose
TBD - created by archiving change audit-fix-docdrift-t1-baseline. Update Purpose after archive.
## Requirements
### Requirement: Rustdoc variant references resolve to real types

Rustdoc that cites an error variant SHALL reference a variant that exists in
the error enum. Phantom variants that were renamed or never existed SHALL be
corrected to the actual variant the function returns.

#### Scenario: claim-check rustdoc references an existing variant

- **GIVEN** `crates/camel-api/src/claim_check.rs` documents the error path of
  its lookup functions in rustdoc
- **WHEN** a reader follows a `CamelError::<Variant>` reference in that rustdoc
- **THEN** the referenced variant exists in `crates/camel-api/src/error.rs`
  (no `NotFound` phantom variant remains)

### Requirement: Stale TODO comments reflect actual implementation state

Source comments marked `TODO(<work-item>)` SHALL either describe open work
accurately or be removed when the referenced work is complete. A TODO that
claims a feature is unimplemented while the feature is wired and consumed is
stale drift and SHALL be corrected.

#### Scenario: hot-reload TODO comments match implemented behavior

- **GIVEN** camel-config wires and the CLI consumes the `config.watch`
  hot-reload flag
- **WHEN** the source is searched for `TODO(CONFIG-004)`
- **THEN** no comment asserts hot-reload is unimplemented or unused

#### Scenario: API-006 re-export TODO is removed, not implemented

- **GIVEN** the trait re-export is a separate API-surface decision outside D1
- **WHEN** the source is searched for `TODO(API-006)`
- **THEN** the stale comment is gone and no re-export of Component/Endpoint/
  Consumer/Producer traits is added by this change

### Requirement: README command and flag listings match shipped surface

Crate READMEs SHALL enumerate the commands and flags the shipped binary
exposes. The `Commands` enum (or equivalent source of truth) is authoritative;
a README that omits shipped commands or flags is drift and SHALL be synced.

#### Scenario: CLI README lists every command variant

- **GIVEN** the `Commands` enum is the source of truth for camel-cli commands
- **WHEN** the camel-cli README top-level command list is read
- **THEN** the list includes every variant, including `plugin` and `openapi`,
  with no malformed section headers

#### Scenario: CLI README documents run-time OTel flags

- **GIVEN** `camel run` accepts `--otel`, `--otel-endpoint`, and
  `--service-name`
- **WHEN** the camel-cli README `camel run` flags section is read
- **THEN** all three flags are documented

### Requirement: DSL step-coverage tables match shipped steps

Crate READMEs that enumerate supported DSL step kinds SHALL cover the steps
the shipped compiler accepts. A step table that omits shipped step variants is
drift and SHALL be synced to the step set the DSL exposes.

#### Scenario: camel-dsl README step tables cover shipped steps

- **GIVEN** the camel-dsl compiler is the source of truth for supported steps
- **WHEN** the camel-dsl README step-coverage tables are read
- **THEN** every shipped step variant appears in the tables, with no shipped
  step omitted

### Requirement: Documentation citations point to live anchors

Comments and docs that cite a work-item anchor (`TODO(PROC-004)` style)
SHALL point to live, relevant context. A citation whose work-item is resolved
or whose pointer is dangling drift SHALL be removed or rewritten to describe
the current state.

#### Scenario: CLI CONTEXT.md carries no dead PROC-004 pointer

- **GIVEN** the camel-cli `CONTEXT.md` referenced `TODO(PROC-004)` for metrics
  instrumentation status
- **WHEN** the comment is reviewed
- **THEN** the pointer is removed or rewritten to reflect the current metrics
  status, with no dangling work-item reference

### Requirement: Documentation matches current behavior

Repository documentation SHALL describe the current behavior, source layout,
feature configuration, and component metric labels without changing runtime
behavior.

#### Scenario: DSL comments explain format-specific null handling

- **GIVEN** the route AST comment describes `parameters: null`
- **WHEN** a reader follows the explanation for serde_json and noyalib
- **THEN** it states that serde_json rejects null up front and noyalib routes
  YAML null through `deserialize_map` to `visit_map` over an empty access,
  while direct-deserializer tests pin the `visit_unit`/`visit_none` arms

#### Scenario: DSL comments do not claim an existing type is omitted

- **GIVEN** `ResequenceStreamYaml` is defined in `route_ast.rs`
- **WHEN** the surrounding comments are read
- **THEN** no comment claims that `ResequenceStreamYaml` is intentionally
  omitted

#### Scenario: Testing build table distinguishes feature sets

- **GIVEN** a reader uses `docs/src/testing/index.md`
- **WHEN** the reader compares the no-default-features and default builds
- **THEN** the table identifies the featureless build separately and reflects
  the current default integration-http and integration-sql features

#### Scenario: Processor catalog citations are source-accurate

- **GIVEN** a processor catalog entry cites `src/lib.rs:N`
- **WHEN** the corresponding module declaration is inspected
- **THEN** N is the actual declaration line for that module

#### Scenario: Component context pages document current b-prime labels

- **GIVEN** the WS, file, or timer component context page is read
- **WHEN** its documented metric labels are compared with source
- **THEN** it documents `b-prime:ws:message-dispatch`,
  `b-prime:file:poll-send`, or `b-prime:timer:fire-send` respectively

#### Scenario: Docs-only scope excludes test harness extraction

- **GIVEN** the docsweep diff is reviewed
- **WHEN** changed paths are enumerated
- **THEN** no component test harness or production implementation is changed

