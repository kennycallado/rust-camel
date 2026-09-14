# Proposal: jobargs

## Why

`camel job` currently treats every `--arg NAME=VALUE` pair as an exchange
header. This prevents job documents from declaring their input contract and
makes interpolation inconsistent between normal jobs and compiled artifacts.
Bd issue `rc-jj8eu` requires a data-only declaration surface that can validate
inputs before boot while preserving existing documents.

## What Changes

- Add a top-level `args:` block as a sibling of `execute:`.
- Support string-only declarations with `required`, `default`, and
  `description`; reject unknown keys at both declaration levels.
- Validate declared `--arg` values, apply defaults, and map failures to exit 2.
- Resolve `${arg:NAME}` through the existing interpolation stage in job and
  compiled-artifact execution paths.
- Keep the no-`args:` header behavior and emit a deprecation note.

Typed arguments, argument registries, traits, and multi-execution are out of
scope.

## Acceptance criteria

- Declared arguments parse only at the document top level and reject malformed
  fields.
- Unknown and missing required arguments fail before boot with exit 2.
- Defaults apply; explicit values win.
- `${arg:NAME}` resolves in `to`, `body`, `headers`, and `timeout` identically
  for `camel job` and compiled artifacts.
- Legacy undeclared `--arg` pairs remain headers with a deprecation note.

## Risk budget

Acceptable risk is limited to the job document parser, CLI validation, and the
shared interpolation seam. No runtime traits, component registries, typed
argument coercion, or unrelated route interpolation behavior may change.
