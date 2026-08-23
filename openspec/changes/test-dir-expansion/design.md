# Design: test-dir-expansion

## Context

`run_tests` (`crates/camel-cli/src/commands/test.rs:49`) iterates its
path slice verbatim. Expansion can slot in before that loop without
touching the runner (`runner.rs`) or document parsing.

## Goals and Non-Goals

- Goal: `camel test <dir>` runs every test document under `<dir>`
  deterministically, without shell globbing.
- Non-goal: glob patterns in arguments (`*.test.yaml` stays the shell's
  job); watch mode; changing FILE-argument semantics.

## Decisions

- **Expand inside `run_tests`, before the document loop.** Expansion
  errors (unreadable directory, zero-document directory) join the
  misuse/parse-error class: reported to `err`, run continues with the
  remaining documents, exit 2.
- **Pure helper, unit-tested in isolation.**
  `expand_test_paths(args: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>)`
  returns (documents, errors) without I/O side effects beyond the walk;
  `run_tests` maps errors to the exit-2 class. Deterministic output:
  each directory's expansion is byte-sorted; across arguments, CLI
  order is preserved.
- **Reuse `is_test_document`.** The suffix predicate from
  `camel-dsl` discovery is the one authority (test-placement-contract
  D-RUN); `camel lint` already consumes it the same way.
- **Exclusions are name-based, any depth**: `target`, `.git`,
  `node_modules`. Nested build artifacts recur (lint-corpus findings);
  skipping them avoids accidental pickup of generated documents.
- **Dedupe by `canonicalize`.** Symlinked or repeated paths collapse;
  first occurrence wins.
- **FILE args stay verbatim.** Only directory arguments expand;
  non-suffixed explicit files keep today's parse-driven behavior.
- **Walk contract:** symlinked directories are not followed (cycle safety);
  non-directory entries whose name matches the test suffix are collected
  regardless of file type.

## Risks / Trade-offs

- Deep trees: unbounded recursion cost is accepted; exclusions keep the
  common cases (target trees) out.
- `canonicalize` failure falls back to raw-path string dedup (never an
  expansion error); the runner's read step owns nonexistent-file errors.

## Migration Plan

None: additive CLI surface; existing invocations unchanged.

## Open Questions

None.
