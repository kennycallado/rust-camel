# Proposal: add-do-try-spec

## Why

OpenSpec has no `do-try` capability spec. The doTry/doCatch/doFinally
semantics lived only in code comments and prose docs. The ambiguity was
not harmless: `DoTrySegment` skipped the finally block when a catch body
failed, citing ADR-0025 invariant #4, while `docs/src/eip/do-try.md`
promised the finally body runs exactly once. bd rc-h3i8d ruled the
Java/Camel behavior (JLS §14.20.2): the finally body runs even when the
catch body fails, and the catch error stays the surfaced failure. The
fix landed on this branch (commit 757ccf2a6). This change records the
corrected behavior as a normative capability spec so the rule survives
refactors.

## What Changes

- Add the `do-try` capability spec with two requirements: finally runs
  exactly once per scope exit, and catch-body failure runs finally and
  surfaces the catch error.
- Six scenarios pin the rule: try success, no catch match, handled
  catch, catch-body failure, failing finally body, Stop in finally.
- No API, builder, DSL, or component changes. The behavior already
  exists on this branch; the spec makes it normative.

## Acceptance criteria

- `openspec validate --all --strict` passes before and after archive.
- Every scenario maps to an existing test in
  `crates/camel-processor/src/do_try_segment.rs` or
  `crates/camel-test/tests/do_try_test.rs`.

## Risk budget

Documentation-only change to `openspec/`; no runtime risk. The
underlying behavior change (rc-h3i8d) is covered by its own tests and
review.

Affected crates: camel-processor (behavior under spec, unchanged by
this change). Bd: rc-h3i8d.
