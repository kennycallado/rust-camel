# 485 verification evidence

Change: `485-dot-ry-dsl-decisions` (expert-gated, `skip_specs: true`)
Worktree: `/home/shared/rust-camel-worktrees/485-dot-ry-dsl-decisions`
Date: 2026-11-01
Environment: `CARGO_TARGET_DIR=$PWD/target`, `TMPDIR=/home/shared/tmp`

## Diff scope (step 2)

`BASE=$(git merge-base HEAD main)` → `42f97521a4e103192decbe28b5c636f5e6484726`

`git diff "$BASE" --name-only`:

```
CONTEXT-MAP.md
docs/adr/0025-outcome-aware-structural-eips.md
docs/adr/0049-workspace-non-exhaustive-policy-for-v1-contract-enums.md
docs/src/concepts/error-handling.md
docs/src/eip/do-try.md
openspec/changes/485-dot-ry-dsl-decisions/.bless.json
openspec/changes/485-dot-ry-dsl-decisions/.openspec.yaml
openspec/changes/485-dot-ry-dsl-decisions/design.md
openspec/changes/485-dot-ry-dsl-decisions/proposal.md
openspec/changes/485-dot-ry-dsl-decisions/tasks.md
```

Plus this new file under `openspec/changes/485-dot-ry-dsl-decisions/evidence/`. No Rust source (`git diff "$BASE" --name-only | rg '\.rs$'` → empty, exit 1) and no `specs/` delta.

## Spec delta check (step 3)

- `openspec/changes/485-dot-ry-dsl-decisions/.openspec.yaml` contains `skip_specs: true`.
- `test ! -d openspec/changes/485-dot-ry-dsl-decisions/specs && echo no-specs` → `no-specs`.

## Gate set (steps 4 and 5)

| command | exit code | output summary |
| --- | --- | --- |
| `cargo fmt --check --all` | 0 | no diff |
| `cargo xtask lint-context-citations` | 0 | `lint-context-citations: OK (0 violations)` |
| `cargo xtask lint-non-exhaustive` | 0 | `lint-non-exhaustive: OK (no violations)` |
| `RUSTDOCFLAGS="-D warnings" cargo doc -p camel-api -p camel-core -p camel-builder -p camel-dsl -p camel-endpoint -p camel-component-validator -p camel-xslt --no-deps` | 0 | docs generated, no warnings-as-errors |
| `cd docs && mdbook test` | 0 | all chapters tested, no failures |
| `openspec validate --all --strict` | 0 | `Totals: 153 passed, 0 failed (153 items)` |
| `cd docs && mdbook build` | 0 | HTML book written to `docs/book`; pre-existing `unclosed HTML tag` warnings in ADR-0024 only |
| `lychee --offline --no-progress book` | not run | `command -v lychee` → empty, exit 1 |

### Link resolution disposition

`lychee` is not installed (`command -v lychee` produced no output, exit 1). Link validation is deferred to CI (`.github/workflows/docs.yml`).

`mdbook build` succeeded and the new `#divergence-from-apache-camel` anchor is present in `docs/book/concepts/error-handling.html` (matched by `id="divergence-from-apache-camel"`), so the cross-reference from `docs/src/eip/do-try.md` resolves at build time. Anchor existence is checked only by CI lychee/mdbook build, not by `mdbook test`.

## Deferred scenario

Deferred scenario ticket: bd `rc-ud3rl` (P4, discovered-from `rc-ho9j1`). The
conductor verified it during planning; it was not created or closed from this
worktree.

Conductor check (run from the repo root, not the worktree):
`bd show rc-ud3rl --json` → `rc-ud3rl status=open priority=4 type=task
title=do-try spec scenarios for CamelExceptionHandled marker rule`.
