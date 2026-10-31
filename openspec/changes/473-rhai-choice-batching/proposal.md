# Proposal: 473-rhai-choice-batching

## Why

A declarative Choice with N `when` clauses compiles each clause to an
independent language predicate (`compile_filter_predicate` → opaque
`PredicateSource::Async` closure). At runtime every clause re-converts the
exchange snapshot (body, headers, properties) and builds a fresh engine
before evaluating its AST. For a 10-clause rhai Choice the snapshot work runs
up to 10 times per message even though clauses are read-only over the same
exchange. The opaque closures cannot batch internally, so the fix needs a new
object-safe group hook in `camel-language-api` (e_gpt ruling on bd rc-1y2qs:
batching is CONDITIONAL on that hook). Follow-up to `rhai-engine-reuse`
(rc-rn6v1).

## What Changes

- `camel-language-api`: add an optional, defaulted `Language::create_predicate_group`
  hook plus an object-safe `PredicateGroup` trait (compile once per group,
  evaluate to first matching clause index or an indexed error). Default
  returns "no group support" so every other language is unaffected.
- `camel-language-rhai`: implement the hook — one engine and one snapshot per
  message, fresh scope and stream counters per predicate (per-clause budgets
  preserved via rhai's per-eval operation counter), strict short-circuit,
  indexed errors, stream-marker bodies keep per-clause conversion.
- `camel-core`: `DeclarativeChoice` compilation selects the group path only
  when ALL `when` clauses share one language that supports the hook; mixed
  languages fall back to the existing sequential path unchanged.
- `camel-processor`: new `ChoiceGroupSegment` (group + per-clause bodies +
  otherwise) mapping the indexed error through the clause's `EvalMeta` via the
  existing `to_expression_failed` carrier.
- Bench-first: extend `camel-processor` choice Criterion bench with 10-clause
  rhai sequential (baseline `choice-pre`), 1-clause control, and mixed-language
  shapes; gate: 10-clause same-language median <= 250 us.
- Docs: Choice/rhai docs state batching + mixed fallback. No user-facing
  config.

Not included (non-goals): Filter/Split context sharing, mixed-language
batching, rhai semantics changes, leaking opaque closures, Filter/Split
grouping.

## Acceptance criteria

- Group hook is object-safe and optional; other languages compile unchanged.
- Choice semantics preserved: clause order, short-circuit (skipped clauses are
  never evaluated), per-clause budgets, fresh scope/stream counters per
  predicate, indexed error/log attribution, redaction, metadata.
- Mixed-language Choice falls back to sequential evaluation.
- Bench gate: 10-clause same-language Choice median <= 250 us vs `choice-pre`;
  no regression on control shapes.
- `cargo test -p camel-processor` and touched crates pass; fmt/clippy/xtask
  lints/docs build/`openspec validate --all --strict` pass.

## Risk budget

Acceptable: new trait surface on `Language` (defaulted, additive); a new
segment type in `camel-processor`. Out of bounds: widening the API beyond the
object-safe hook, changing PredicateSource, sharing mutable context between
clauses, gorilla-tuning the 250 us gate. If the hook cannot stay object-safe,
park with a design note.

Affected crates: `camel-language-api`, `camel-language-rhai`, `camel-core`,
`camel-processor`. Bd: rc-1y2qs.
