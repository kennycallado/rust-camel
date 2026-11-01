# Proposal: 485-dot-ry-dsl-decisions

## Why

Three decision/debt tickets from missions 479 and 483 are still open. Each asks
for a recorded decision, not for a code change:

- **rc-zaqjd** — mission 479 made a `Handled`/`Continued` doTry catch clear the
  exchange error through `clear_error()`. That also removes `CamelExceptionCaught`,
  so a downstream step no longer sees the caught exception. Apache Camel retains
  `EXCEPTION_CAUGHT` after a handled catch. Decision: restore the property, or
  record the divergence.
- **rc-ho9j1** — the builder `doTry` runtime stopped setting `CamelExceptionHandled`
  when `DoTryService` was deleted. The property is Camel-compatibility only; the
  runtime keys control flow off `PipelineOutcome`. The canonical rule is not yet
  written down.
- **rc-b48du** — mission 483 added internal `ResponseMarshal` variants to the
  public `RouteDslStep` and `DeclarativeStep` enums. Neither enum is
  `#[non_exhaustive]`, so the next internal variant is a semver break. Decision:
  adopt `#[non_exhaustive]`, or freeze the ASTs and document the release rule.

## What Changes

Docs and ADRs only. No runtime behavior changes.

- ADR-0025 gains an amendment for exception-property retention after a handled
  catch (rc-zaqjd) and the canonical `CamelExceptionHandled` rule (rc-ho9j1).
- `docs/src/concepts/error-handling.md` gains a "Divergence from Apache Camel"
  note for the caught property and the handled-marker rule.
- `docs/src/eip/do-try.md` gains a short user-facing pointer to both rules.
- ADR-0049 gains a scope clarification: `RouteDslStep` and `DeclarativeStep` stay
  exhaustive by design; `DeclarativeStepKind` is the intended external
  discriminant; a new variant is a breaking change and must appear in release
  notes (rc-b48du).
- `CONTEXT-MAP.md` summaries for ADR-0025 and ADR-0049 record the amendments.
- No CHANGELOG file exists. `cargo xtask changelog` renders release notes from
  commit subjects, so the AST-variant rule is recorded in ADR-0049 and enforced
  by release discipline.

## Spec decision: no delta

`skip_specs: true`. The do-try spec already settles the behavior this change
documents, and it explicitly leaves the caught-property decision open:

> "The retention of `CamelExceptionCaught` after a handled catch is a separate
> decision and SHALL NOT be settled by this requirement."
> — `openspec/specs/do-try/spec.md`, requirement *A Handled or Continued catch
> exits with the error cleared*

The same spec already pins that `Handled` sets `CamelExceptionHandled` and
`Continued` does not (`Scenario: Handled catch clears the error and marks it
handled`; `Scenario: Continued catch clears the error without the handled
marker`). ADR-0025 records the decision that the spec defers, so no requirement
is added, removed, or contradicted. Normative scenarios for marker persistence
across nested blocks are deferred to bd **rc-ud3rl** (P4, discovered-from
`rc-ho9j1`).

Excluded: restoring `CamelExceptionCaught`, setting `CamelExceptionHandled` in
the `Continued` arm, adding `#[non_exhaustive]`, any handler-semantics change,
retry behavior, and spec deltas.

## Acceptance criteria

- ADR-0025 records the caught-property divergence and the handled-marker rule.
- ADR-0049 records why the two DSL AST enums stay exhaustive and the release
  rule for new variants, including internal/doc-hidden ones.
- `docs/src/concepts/error-handling.md` and `docs/src/eip/do-try.md` state the
  user-facing rules and link the ADRs.
- `CONTEXT-MAP.md` summaries reflect both amendments.
- No Rust source file changes; `git diff "$(git merge-base HEAD main)" -- '*.rs'`
  is empty (merge-base, not the moving main tip).
- `cargo xtask lint-context-citations`, `cargo xtask lint-non-exhaustive`,
  rustdoc `-D warnings`, `mdbook test`, and
  `openspec validate --all --strict` pass. Link resolution uses the CI command
  `mdbook build docs && lychee --offline --no-progress docs/book`
  (`.github/workflows/docs.yml`); lychee is not installed locally, so that step
  is deferred to CI with evidence.

## Risk budget

Zero runtime risk: no code changes. The residual risk is documentation drift
between the ADRs and the do-try spec; the ADR cites the spec clause it decides.
The DSL AST semver risk stays accepted and is recorded in ADR-0049 rather than
removed.
