# Proposal: unify-builder-error-policy

## Why

`camel-builder` exposes an inconsistent error policy on its public fluent API. The four
terminal/format methods (`build`, `build_canonical`, `marshal`, `unmarshal`) return
`Result<_, CamelError>`, but two misuse paths **panic**:

- `DoTryBuilder::do_finally(self)` (`do_try.rs:104`) panics on a second call in the same
  scope (`end_do_finally().do_finally()`).
- `DoCatchBuilder::disposition(value)` (`do_try.rs:151`) panics when passed
  `ExceptionDisposition::Continued`, even though the sugar methods `handled()` /
  `propagate()` already cover the two supported dispositions.

Both panics are intentional and covered by `#[should_panic]` tests today, and
`crates/camel-builder/CONTEXT.md` records the asymmetry as audit finding I1 ("decision
noted, not prescribed"). This change prescribes and applies the fix. It is time-boxed:
post-1.0, `panic!` ↔ `Result` is a breaking signature change, so the pre-freeze window is
the moment to unify. Tracked in bd as `rc-0lhn` (Important, pre-freeze).

## What Changes

**In scope** (`crates/camel-builder/src/do_try.rs`):

- `DoCatchBuilder::disposition` — **remove** the general method. Keep the sugar methods
  `handled()` / `propagate()`, which set the disposition field directly. `Continued`
  becomes unrepresentable at the type level (idiomatic Rust: make invalid state
  impossible). Zero external callers (verified: `rg '\.disposition\('` across `crates/`,
  `examples/`, `tests/` returns hits only inside `do_try.rs`: the two sugar method bodies
  — which are rewritten to set the field directly — and two test sites, one deleted and
  one switched to `.handled()`).
- `DoTryBuilder::do_finally` — change signature to
  `do_finally(self) -> Result<DoFinallyBuilder, CamelError>`. On a second call, return
  `Err(CamelError::RouteError(...))` (the variant `build()` already uses for route
  construction errors). Type-level prevention would require a real typestate machine
  that the builder deliberately avoids (CONTEXT.md "Architecture notes").

**Policy established** (recorded in `CONTEXT.md`):

> Builder public APIs do not panic on user-reachable misuse. Prefer type-level prevention
> where cheap; fall back to `Result` where type-level prevention would require a typestate
> machine. Never panic on user input/state.

This generalizes for the T1 sweep (cross-crate panic-vs-Result invariant).

**Out of scope**: the M4 "canonical v1" stale string, the M3 `StepAccumulator` sealing,
and any `do_finally` typestate redesign. Those stay tracked separately.

## Acceptance criteria

- No public method on `camel-builder`'s fluent API panics on user-reachable misuse
  (`rg 'panic!\(' crates/camel-builder/src/` returns only test assertions and
  genuinely-unreachable `unreachable!()` sites with a documenting comment).
- `disposition` is gone; `handled()` / `propagate()` remain and set the field directly.
- `do_finally` returns `Result<DoFinallyBuilder, CamelError>`; a second call yields
  `Err(CamelError::RouteError(_))` with a message naming the misuse.
- The two `#[should_panic]` tests are replaced by positive `Result`-assertion (double
  `do_finally`) / removed (Continued is now unrepresentable) tests.
- The two external single-call `do_finally()` callers
  (`examples/do-try/src/main.rs`, `crates/camel-test/tests/do_try_test.rs`) compile and
  their tests pass.
- All 160 existing `camel-builder` tests still pass; `CONTEXT.md` panic-vs-Result note is
  updated from "decision noted, not prescribed" to the prescribed policy above.

## Risk budget

- **Breaking API change** (signature + method removal) — **accepted** because this is
  pre-1.0 and the whole point of the change. Blast radius verified small: 2 external
  `do_finally` callers (both single-call, both updatable to `?`), 0 external `disposition`
  callers.
- **camel-test blast radius** — `camel-builder` is a dep of `camel-test` (25+ files), but
  only `do_try_test.rs` touches `do_finally`; the rest use the `Result`-returning
  terminals unaffected by this change.
- **Out of bounds**: no runtime/EIP behavior change (camel-builder only constructs route
  specs; L7 N/A per CONTEXT.md), no `CamelError` enum reshaping (reuse `RouteError`), no
  new deps.
