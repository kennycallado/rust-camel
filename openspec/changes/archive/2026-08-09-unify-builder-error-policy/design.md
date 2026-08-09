# Design: unify-builder-error-policy

## Approach

Unify `camel-builder`'s public error policy so no fluent-API method panics on
user-reachable misuse. Two sites, two mechanisms — chosen per site because the
*nature* of the misuse differs:

**Site A — `DoCatchBuilder::disposition` (type-level prevention).** Remove the general
`disposition(value: ExceptionDisposition) -> Self` method. Keep the sugar methods
`handled()` / `propagate()`, rewritten to set the private `disposition` field directly
(`self.disposition = ExceptionDisposition::Handled; self`). `Continued` becomes
unrepresentable: there is no method that accepts it, so the panic path is deleted rather
than converted. This is the audit's preferred direction (Opción A) and the idiomatic Rust
choice ("make invalid state impossible"). Blast radius is zero — `rg '\.disposition\('`
across `crates/`, `examples/`, `tests/` returns hits only inside `do_try.rs`: the two
sugar method bodies (handled/propagate, rewritten to set the field directly) and two test
sites (the `#[should_panic]` test that this change deletes, and the shape test which
switches to `.handled()`).

**Site B — `DoTryBuilder::do_finally` (`Result`).** Change the signature to
`do_finally(self) -> Result<DoFinallyBuilder, CamelError>`. On a second call (detected via
the existing `finally_set` flag), return
`Err(CamelError::RouteError("do_finally can only be called once per do_try scope".into()))`.
`RouteError(String)` is reused — it is the variant `build()` and `build_canonical()`
already use for route-construction errors, so no `CamelError` enum change is needed and the
misuse error is matchable by the same discriminant callers already handle. Type-level
prevention was rejected here (see Alternatives).

**Test rework** (`do_try.rs` inline `mod tests`):

- `disposition_continued_panics` (`#[should_panic]`) — **delete**. The misuse is now
  impossible; a runtime test cannot assert the absence of a method. Instead, add a
  `compile_fail` doctest on the `handled()` / `propagate()` sugar methods that references
  `.disposition(ExceptionDisposition::Continued)` — since the method no longer exists, the
  doctest fails to compile, mechanically proving `Continued` is unrepresentable. This is
  the regression guard.
- `do_finally_called_twice_panics` (`#[should_panic]`) — **rewrite** as
  `do_finally_called_twice_returns_err`: build the double-call chain, assert the result is
  `Err(CamelError::RouteError(msg))` and the message contains the misuse hint.
- `do_try_builder_assembles_correct_shape` (line 236 calls `.disposition(Handled)`) —
  switch to `.handled()`.
- `do_try_builder_disposition_sugar_methods` — unchanged (already uses sugar).

**External caller updates** (2 sites, both single-call valid paths):

- `examples/do-try/src/main.rs:103` and `crates/camel-test/tests/do_try_test.rs:131` —
  `do_finally()` now returns `Result`, mid-chain. Each call site is rewritten to bind the
  child builder (`let f = scope.do_finally()?;` then `f.process(...).end_do_finally()...`)
  or, where the enclosing function already returns `Result`, to use `?`. The enclosing
  signatures are updated where needed.

**Doc + CONTEXT.md update.** The `# Panics` doc blocks on both methods are removed; the
`do_finally` doc gains a `# Errors` block. `crates/camel-builder/CONTEXT.md`'s
"panic-vs-`Result` policy" note is rewritten from "decision noted, not prescribed" to the
prescribed policy (see proposal), and the `propagate()` doc comment is updated (it
currently references the removed `disposition` method).

**Mechanical enforcement.** The "no panic on misuse" policy is checked mechanically, not
just by prose: `cargo clippy -p camel-builder --lib -- -D clippy::panic` SHALL pass (the
`--lib` scope excludes `#[cfg(test)]` assertion panics; after this change no `panic!`
remains in the library target). The `compile_fail` doctest above mechanically proves the
`disposition` method is gone. Both checks are added to the change's acceptance gate.

## Affected crates

- **`camel-builder`** (`crates/camel-builder/src/do_try.rs`, `CONTEXT.md`) — the only
  crate with source changes. `RouteError` is consumed from `camel-api` (re-exported, no
  change).
- **`camel-test`** (`crates/camel-test/tests/do_try_test.rs`) — one test file updated for
  the `do_finally` `Result` signature.
- **`examples/do-try`** (`examples/do-try/src/main.rs`) — one example updated for the same
  signature change.

## Architecture boundaries

camel-builder is a **route-authoring front end** (per CONTEXT.md: "Not a runtime EIP
crate, L7 N/A"). It constructs `RouteDefinition` / `CanonicalRouteSpec`; it implements no
pipeline runtime behavior. This change touches only the **authoring surface**
(signatures + docs), not the data plane (`Service<Exchange>`) or the control plane
(`RuntimeCommandBus`). ADR-0001 (data/control plane split) is respected trivially — no
runtime or lifecycle code is touched. ADR-0011 (strict rejection of unsupported fields via
`Result`) is *strengthened*: the policy now covers misuse paths, not only unsupported
fields. No ADR amendment is required; the policy is recorded in crate-local `CONTEXT.md`
(following the camel-config DP-9 / camel-dsl DP-8 precedent for crate-local decisions).

## Alternatives considered

1. **`Result` for both sites (uniform mechanism).** Rejected for `disposition`: the method
   accepts a type wider than it supports (the audit's root smell), and zero external
   callers exist. Converting to `Result` would preserve the smell; removing the method
   eliminates it at the type level. Uniformity of *outcome* (no panic) matters more than
   uniformity of *mechanism*.
2. **Typestate for `do_finally` (prevent double-call at compile time).** `end_do_finally()`
   would return a distinct type lacking `do_finally()`. Rejected: the double-call is
   reachable through `end_do_finally().do_catch(...).end_do_catch().do_finally()`, so the
   "finally-set" state would have to thread through `DoCatchBuilder` and every other
   method that hands back a `DoTryBuilder`. That is a real typestate machine, which
   CONTEXT.md explicitly says the builder avoids ("without a real typestate machine"). The
   cost exceeds the benefit for a single misuse path.
3. **Defer to post-1.0 (audit Opción C).** Rejected — signature changes are breaking, so
   deferring locks the panic in. The bd issue is flagged pre-freeze for exactly this
   reason.
4. **New `CamelError::BuilderMisuse` variant.** Rejected — `RouteError(String)` already
   covers route-construction problems and is what `build()` returns for adjacent misuse:
   missing/empty `route_id` (`lib.rs:702`), mixed error-handler modes (`lib.rs:712`), and
   what `build_canonical()` returns for unsupported canonical steps. A new variant would
   fragment the error space without aiding callers, who match on `RouteError` already.
   (Note: duplicate route IDs are NOT detected by `build()` — that happens later at
   `CamelContext::add_route_definition` time, per the comment at `lib.rs:695-696`.)
