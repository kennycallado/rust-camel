# Proposal: assurance-report-hardening

## Why

Monthly run `41999707610` (2026-11-05) failed without firing its own alarm: the
`mutants-baseline` job lost runner comms and uploaded no artifact, and the
`report` job's "Build issue body" step resolved evidence with
`find art-mutants-baseline -name outcomes.json` under `set -euo pipefail`. `find`
exits non-zero when the directory does not exist, so the step died before the
"Fail only on infra (missing evidence)" guard ran and before `gh issue create`
executed. Result: a red run, **no durable issue**, no alarm — the exact failure
the report job exists to escalate. Body construction lives inline in YAML, so
none of this was testable offline. (bd rc-g3nls, P2.)

## What Changes

- Extract issue-body construction from `.github/workflows/assurance-monthly.yml`
  into `scripts/assurance-report-body.sh` (offline, no `gh`, no network):
  failure-tolerant evidence lookup, explicit `MISSING ... (infra failure)`
  sections per missing piece, env-file output for the guard inputs
  (`OUT_BASE`, `OUT_ROT`, `FUZZF`, plus a single `INFRA_MISSING` list).
- Harden the `report` job: document that the bare `download-artifact@v8`
  download-all form tolerates zero artifacts, call the script for body
  construction, create the issue even with zero artifacts (that IS the alarm),
  and drive the existing fail-only-on-infra guard from `INFRA_MISSING`.
  Issue publication becomes its own step ("Publish issue"), split from body
  construction: `$GITHUB_ENV` writes apply to *subsequent* steps only, so the
  publish step consumes `STAMP` and the evidence paths from the env file the
  body step emitted. The publish step also fixes a second, latent abort on the
  same failure shape: the current-survivors pipeline
  (`sed -n ... | grep -v '<!--'`) exits 1 under `set -euo pipefail` when the
  baseline survivor set is empty, killing publication whenever a previous
  issue exists — empty survivor sets are valid and must publish.
- Add `scripts/tests/assurance-report-body-test.sh`: an offline battery
  covering all-present, baseline-missing, fuzz-missing, rotation-missing,
  zero-artifacts (the 41999707610 shape), marker preservation, malformed
  present JSON (real error), unwritable output (real error), and usage errors.
- **No spec deltas** (`skip_specs: true`): CI report-job behaviour is not a
  canonical capability of the Rust framework; there is nothing in
  `openspec/specs/` this change extends. The contract that matters
  (findings never fail; only missing evidence does; markers byte-identical) is
  preserved verbatim and is guarded by the new test battery instead.

Explicitly excluded: the three tool jobs, rotation config, thresholds, issue
template wording beyond what the extraction requires, runner-comms flake,
dependency files (`Cargo.lock` — other missions own them today).

## Acceptance criteria

- New script exits 0 on an empty artifacts root, emits a body with the
  `# assurance-monthly:` header, all three job results, and all three MISSING
  lines; `INFRA_MISSING` is set.
- Only real errors (unwritable output, malformed JSON in a present file,
  usage errors) exit non-zero, each with an actionable message.
- `<!--baseline-data-start-->`/`<!--baseline-data-end-->` markers and their
  sorted `.name` line form are byte-identical to today's output.
- Test battery passes offline; workflow YAML parses; no `*.rs`/`Cargo.toml`
  changes.
- The publish path reaches `gh issue create` even when a previous
  assurance-monthly issue exists and the current survivor set is empty
  (zero-artifacts run, or all evidence present with zero baseline survivors) —
  covered offline with a stubbed `gh` executing the workflow's publish-step
  shell.
- `STAMP` reaches the issue title via subsequent-step `$GITHUB_ENV`
  semantics (publish step separate from body step); the offline wiring check
  asserts the stubbed `gh` received the populated `--title`.

## Risk budget

Low: one workflow's report job plus two new scripts. Main risk is byte-drift in
the marker block (next month diffs against it) — mitigated by a dedicated
marker-preservation test. Out of bounds: touching tool jobs, thresholds,
rotation config, or any Rust crate.

## Affected crates

None. CI workflow + shell scripts + shell tests only.
