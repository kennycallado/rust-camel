# Proposal: scenario-sql-diagnostics

## Why

Scenario SQL actions appear as generic `action` rows in `camel test` output.
The scenario console also prints SQLx DEBUG statements, including seeded literals.
The combined HTTP partner and SQL example still carries an unverified marker.
Beads: rc-mjkgp, rc-zmpbp, rc-y15du.

## What Changes

- Label SQL action rows `sql` in `camel-cli`.
- Set the scenario console's SQLx minimum severity to INFO in `camel-integration-test`.
- Preserve all levels from other targets and all events in the capture layer.
- Keep the scenario subscriber independent of ambient configuration.
- Execute the combined example before removing its verification markers.
- Correct the missing normative verb in the existing CLI flavor spec.

The affected contexts are the CLI composition root and the scenario harness.
[CONTEXT-MAP](https://github.com/kennycallado/rust-camel/blob/main/CONTEXT-MAP.md) defines these boundaries.
No dependency, datasource, Runtime, DSL, or component behavior changes are needed.

## Acceptance criteria

- SQL actions produce `scenario[i] sql` rows.
- Ordinary SQLx DEBUG statements do not appear on the scenario console.
- SQLx DEBUG events still reach document-level `logs:` assertions.
- SQLx INFO, WARN, and ERROR events remain eligible for console output.
- Ambient `RUST_LOG` does not alter the fixed scenario policy or captured assertions.
- The combined fixture completes all seven actions before its verification markers are removed.
- The corrected CLI flavor requirement passes strict OpenSpec validation.

## Risk budget

The console becomes quieter for SQLx DEBUG and TRACE events only.
Slow-query WARN events can still contain statements. This change defines verbosity, not SQL redaction.
Unexpected fixture or pool failures require diagnosis and a separate scope decision.
