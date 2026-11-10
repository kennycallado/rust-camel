## MODIFIED Requirements

### Requirement: docs.yml rustdoc runs the registry crate set

The `Documentation` workflow's "Build Rustdoc" step SHALL document exactly
the crates named by the `doc-build` gate in `AGENTS.md ## QUALITY GATES`,
with `--no-deps` and warnings denied via `RUSTDOCFLAGS="-D warnings"`.
The step SHALL carry a comment naming `AGENTS.md doc-build` as the source
of truth and referencing bd rc-o8pxk.

#### Scenario: Regression in a documented crate fails CI

- **GIVEN** the registry command documents eight crates including
  `camel-component-api`
- **WHEN** a change on main breaks rustdoc (for example a broken
  intra-doc link) in any of the eight crates
- **THEN** the `docs.yml` `validate` job fails with the rustdoc error
  instead of staying green while the local sweep fails

#### Scenario: Registry and workflow disagree

- **GIVEN** a change that adds or removes a crate from the `doc-build`
  registry command in `AGENTS.md`
- **WHEN** the same change does not update the "Build Rustdoc" step to
  the same crate set
- **THEN** `cargo xtask lint-gate-parity` exits 1 naming the
  `doc-build` gate with the paste-ready expected and actual commands,
  failing the local sweep and the change before merge

Enforced mechanically by the `lint-gate-parity` xtask gate (see the
`gate-parity-lint` capability).
