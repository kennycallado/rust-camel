# Proposal: test-result-integrity

## Why

A red or cancelled CI run must not look like a green suite, and a release
must not ship an untested SHA. Two measured defects (e_opus audit:
`.opencode/fleet/inbox/reference/test-count-audit.md`, bd rc-9wx6u +
rc-1x9fl):

1. **Fail-fast undercount.** `ci.yml` runs `cargo test --workspace` without
   `--no-fail-fast`, so on a red run the first failing binary aborts the
   rest. Green runs execute 636 test targets; measured red runs reached
   only 41 / 37 / 309 targets (runs 41528349507, 41801330337,
   41654602812) — as little as 37 of 636 targets (~5.8% of the suite) —
   with cargo printing
   only `error: test failed, to rerun pass ...` and no count of what never
   ran. Same shape on the exec-flavor leg (`ci.yml:255`).
2. **Silent skip + ungated release.** In `full-tests-linux`, a failing
   earlier step (JMS: runs 42490710101, 42470660723) leaves the workspace
   test step *skipped* — 0 of 636 targets run — while an unrelated nextest
   line still reports 13,105 passed for `--lib` only. Cancelled Full Tests
   runs are frequent. `release.yml` (tag-triggered) runs no tests and
   checks no CI result, so a tag can build, publish crates.io, and cut a
   GitHub Release from a SHA whose Full Tests never ran or never passed.

## What Changes

- `ci.yml` (`full-tests-linux`): add `--no-fail-fast` to
  `Test (full workspace)` and to `Test camel-cli (exec flavor)`, with the
  rc-97uah-style budget comment (a red run is then *expected* to take
  about as long as a green one, ~85–115 min — an estimate, subject to
  successful compilation; the 150-minute job timeout is a hard cap, not
  a completion guarantee).
- `ci.yml` (`full-tests-linux`): add `if: ${{ !cancelled() }}` to the same
  two steps so a failing earlier step no longer silently skips them; the
  job conclusion keeps its meaning: success ⇒ the tests ran and passed.
- `release.yml`: add a fail-closed `ci-preflight` job that, before the
  matrix and publish run, requires a *successful, completed push run on
  main* of `ci.yml` for the exact tagged commit whose
  `Full Tests (ubuntu-latest)` job — and its two test steps — concluded
  `success`. PR-triggered runs do not qualify as evidence (they test
  synthetic merge commits). No qualifying run, a cancelled run, a
  skipped step, or any API failure ⇒ the release fails. Wired with
  `needs` so the preflight gates `call-release-matrix` (and thus
  `publish`) before any side effect. Token: built-in `GITHUB_TOKEN` with
  `actions: read`; no secret setup.
- `.opencode/prompts/bump-version.md`: a checklist step stating that a
  skipped or cancelled Full Tests run is *not tested*, with the exact
  `gh` command and what to look for. Rule 0b in the Cargo.toml comment is
  untouched.

Excluded: no `crates/**/src/**` changes, no test behaviour changes, no
dependency changes, no matrix input / artifact-name / publish-step edits
beyond the preflight and its `needs`, no scheduled duplicate CI.

## Acceptance criteria

- Both test steps in `full-tests-linux` carry `--no-fail-fast` and
  `if: ${{ !cancelled() }}`, each with a comment stating the budget
  estimate and the rc ids.
- A local scratch-workspace demonstration shows: without the flag, one
  failing binary stops the run and later targets do not execute; with
  `--no-fail-fast`, every target executes and cargo lists all failed
  targets. (CI itself cannot run locally — the workflow-level behaviour
  is verified by inspection + parse, and the exact post-merge human
  verification command is stated in design.md.)
- `release.yml` fails closed: for a tag whose commit has no successful
  push-on-main CI run, or whose Full Tests job/steps did not all conclude
  `success`, the preflight exits nonzero and neither matrix nor publish
  starts. PR-triggered runs alone never satisfy the check.
- The preflight command, the JSON fields it reads, and the `actions: read`
  token scope appear in the workflow comment and in the checklist.
- Every edited workflow parses as YAML; `bash scripts/gates-local.sh`
  exits 0.

## Risk budget

- Acceptable: a red Full Tests run is expected to cost about as much as
  a green one (~85–115 min estimate; completion still requires
  successful compilation, and the 150-min job timeout is a hard cap,
  not a completion guarantee) — that is the price of a complete failure
  list (accepted precedent: rc-97uah on the weekly job).
- Acceptable: after a failing *setup* step, the workspace test step runs
  and fails again at compile (bounded by the job timeout); the job was
  already red, so the conclusion is unchanged and the signal grows.
- Acceptable: tags cut from SHAs with no green CI run (e.g. hotfix tags
  off unmerged branches) now fail the release until CI is green — this is
  the intended fail-closed behaviour, not a regression.
- Out of bounds: weakening any gate (every change must make a red or
  untested path *fail*, never pass), touching matrix inputs/artifact
  names/publish steps, moving Full Tests to nextest (no doctests),
  adding summary-parsing steps, scheduled duplicate CI.
