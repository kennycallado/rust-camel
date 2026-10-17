## MODIFIED Requirements

### Requirement: path-filtered PR smoke trigger

The `fuzz-smoke.yml` workflow SHALL run on `pull_request` events whose
changed files match at least one of: `crates/camel-dsl/**`, `fuzz/**`,
`scripts/xtask/**`, `.github/workflows/fuzz-smoke.yml`, `Cargo.toml`,
`Cargo.lock`; and on manual `workflow_dispatch`. It SHALL NOT run on
pushes to `main`. Within a run, the workflow SHALL select the smoke
legs by applying these ordered rules to the changed-path set, combining
selections by union:
`fuzz/seeds/<target>/**` selects `<target>` for each of `dsl_yaml`,
`dsl_json`, `dsl_template`, `dsl_parity`, `dsl_rest`, `dsl_mcp`,
`dsl_openapi`;
`crates/camel-dsl/src/yaml.rs` selects `dsl_yaml`, `dsl_parity`,
`dsl_rest`, `dsl_mcp`, and `dsl_openapi`;
`crates/camel-dsl/src/json.rs` selects `dsl_json`, `dsl_parity`,
`dsl_rest`, `dsl_mcp`, and `dsl_openapi`;
`crates/camel-dsl/src/template/**` selects `dsl_template` and
`dsl_parity`; `crates/camel-dsl/src/rest.rs` selects `dsl_rest` and
`dsl_openapi`; `crates/camel-dsl/src/mcp.rs` selects `dsl_mcp`;
`crates/camel-dsl/src/openapi.rs` selects `dsl_openapi`; any other
changed path matching the workflow trigger selects all legs.
`workflow_dispatch` SHALL run all legs. (The front-end rules include
the three channel legs because every channel harness drives both
front-ends.)

#### Scenario: workflow triggers on a PR touching the wrapper

- **WHEN** a PR modifies `scripts/xtask/src/fuzz.rs`
- **THEN** the `fuzz-smoke` workflow runs for that PR with all legs
  selected

#### Scenario: workflow skips unrelated PRs

- **WHEN** a PR modifies only `crates/camel-core/**`
- **THEN** the `fuzz-smoke` workflow does not run

#### Scenario: workflow triggers on root manifest drift

- **WHEN** a PR modifies `Cargo.lock` only
- **THEN** the `fuzz-smoke` workflow runs for that PR

#### Scenario: front-end changes select their leg plus the channel legs

- **WHEN** a PR modifies `crates/camel-dsl/src/json.rs`
- **THEN** the run smokes `dsl_json`, `dsl_parity`, `dsl_rest`,
  `dsl_mcp`, and `dsl_openapi`, and skips `dsl_yaml` and `dsl_template`

#### Scenario: channel source changes select the channel legs

- **WHEN** a PR modifies `crates/camel-dsl/src/rest.rs`
- **THEN** the run smokes `dsl_rest` and `dsl_openapi` and skips the
  other legs

- **WHEN** a PR modifies `crates/camel-dsl/src/mcp.rs`
- **THEN** the run smokes `dsl_mcp` and skips the other legs

#### Scenario: seed-only change selects its leg

- **WHEN** a PR modifies `fuzz/seeds/dsl_mcp/anything.json`
- **THEN** the run smokes `dsl_mcp` and skips the other legs

#### Scenario: manual drill run

- **WHEN** a maintainer dispatches the workflow from the Actions tab
- **THEN** the drills execute on the selected ref with all legs
