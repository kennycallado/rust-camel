# Builder

The programmatic route-authoring layer: a fluent Rust API that constructs a
`RouteDefinition` (and, on the canonical path, a `CanonicalRouteSpec`) by method chaining.
camel-builder is the primary way to define routes in Rust code; the declarative YAML/JSON
form lives in camel-dsl. camel-builder qualifies for a crate-local `CONTEXT.md` under the
CONTEXT-MAP.md coverage policy — it is **user-visible** (consumed by 30+ callers across
`camel-test/tests/*`, `camel-test/src/harness.rs`, `camel-core`/`camel-processor` READMEs,
`examples/*`, and 14+ component READMEs) and **operationally surprising** (a mixed
panic-vs-`Result` policy on the public surface and child builders
that carry ownership of their parent).

## Language

Crate-specific vocabulary. Cross-cutting terms (Exchange, Message, RouteDefinition,
CanonicalRouteSpec, ErrorHandler) live in CONTEXT-MAP.md and `camel-dsl/CONTEXT.md` and are not
redefined here.

**RouteBuilder**:
The fluent entry point (`struct RouteBuilder`, `crates/camel-builder/src/lib.rs:358`). `RouteBuilder::from(uri)`
opens a route; chained methods take `mut self` / `self` and return `Self` or a child builder,
so a route is built in a single expression and the builder is consumed by `build()` /
`build_canonical()`. **Programmatic and Rust-side** — do **not** confuse it with
`RouteDslRoute` / `RouteDslRoutes` (camel-dsl, the declarative YAML/JSON authoring AST,
ADR-0017). Both forms lower to the same `RouteDefinition`; camel-builder is the imperative
writer, camel-dsl is the declarative parser. The term is glossary-owned **here**, not in
camel-dsl.
_Avoid_: builder, route factory, DSL builder (ambiguous with camel-dsl)

**StepAccumulator**:
Public trait (`trait StepAccumulator`) that supplies the chainable step-appending methods (`to`,
`set_header`, `log`, ...) via **default implementations** over a single required
`steps_mut(&mut self) -> &mut Vec<BuilderStep>`. Un-sealed by design: it has no external
implementors in the workspace (all uses are `use ...StepAccumulator` to bring the default
methods into scope), and it is intended to grow **default-only** so additions stay
non-breaking. Adding a *required* method post-1.0 would break external implementors — a
residual risk accepted while the design stays default-only.
_Avoid_: step trait, accumulator, StepAccumulation (not a symbol — the real name is
`StepAccumulator`)

**BuilderStep**:
Re-exported from `camel_core::route::BuilderStep` (`enum BuilderStep`); **not** owned by this crate.
camel-builder is the canonical *writer* of `BuilderStep` values through the fluent API, while
camel-core owns the type. A `BuilderStep` is the intermediate form each fluent call appends;
`build()` lowers the accumulated `Vec<BuilderStep>` to a `RouteDefinition`.
_Avoid_: step, builder instruction, BuildStep

**Child builder (FilterBuilder / ChoiceBuilder / SplitBuilder / MulticastBuilder /
ThrottleBuilder / LoopBuilder / LoadBalancerBuilder / OnExceptionBuilder / DoTryBuilder /
DoCatchBuilder / DoFinallyBuilder)**:
A branching sub-builder that **takes the parent `RouteBuilder` by value** (`parent:
RouteBuilder`, e.g. `struct OnExceptionBuilder`) and whose `.end_*()` returns the parent. This is
**typestate-via-parent-ownership**: the compile-time ordering guarantee comes from move
semantics (the parent is unavailable until `.end_*()` hands it back), not from a real
typestate encoded in the type parameters. State lives in the struct, not in the type.
_Avoid_: sub-builder, nested builder, typestate builder (there is no type-level state
machine)

**build() / build_canonical()**:
The two terminal methods. `build()` (`fn build`) produces a full `RouteDefinition`.
`build_canonical()` (`fn build_canonical`) produces a `CanonicalRouteSpec` **version 2** (ADR-0016;
`tests/canonical_spec_test.rs` asserts `spec.version == 2`) and supports a **subset** of steps
— unsupported steps are strictly rejected with a `CamelError` (no silent loss), consistent
with ADR-0011.
_Avoid_: compile, finalize, to_route

## `#[non_exhaustive]` posture (crate-local)

camel-builder is **out of ADR-0049's mandatory scope** — that policy binds only the three
contract crates (`camel-api`, `camel-component-api`, `camel-language-api`). The posture below
resolves to **N/A by category**, following the same crate-local reasoning applied in
camel-config (DP-9, `0a720767`) and camel-dsl (DP-8, `98ace84e`).

| Kind | non_exhaustive | Rationale (ADR-0049 §Scope) |
|------|----------------|------------------------------|
| `pub enum` | **N/A — none exist** | `rg 'pub enum' crates/camel-builder/src/` → **0 hits** (verified HEAD `7f9d8a03`). ADR-0049's forced-`_ =>`-arm cost applies only to contract enums; camel-builder exposes none, so the policy is inapplicable by category, not by omission. |
| `pub struct` (16: 13 in `lib.rs`, 3 in `do_try.rs`) | **No** | Builder structs are constructed only through the fluent API (`RouteBuilder::from(...)` + child-builder accessors), never as external struct literals, so `#[non_exhaustive]` would add no forward-compat value. |

Counts (verified mechanically, HEAD `7f9d8a03`): **16** `pub struct`, **1** `pub trait`
(`StepAccumulator`), **0** `pub enum`; **4520** LOC in `src/` (`lib.rs` 4211, `do_try.rs`
309); **160** tests pass (151 inline + 4 `canonical_spec_test` + 5 `log_eip_test`).

## Architecture notes

**Fluent construction — consuming-self + child-builder-parent-ownership.**
Root chaining methods take `self` and return `Self`; branching methods move the parent into a
child builder and hand it back on `.end_*()`. The design gives compile-time step ordering
without a real typestate machine (see *Child builder* above). This is why `RouteBuilder`
lives in its own crate distinct from the declarative camel-dsl AST: the imperative and
declarative authoring forms are separate front ends that converge on `RouteDefinition`.

**panic-vs-`Result` policy (mixed — decision noted, not prescribed here).**
As of HEAD `7f9d8a03`, the terminal and format methods return `Result<_, CamelError>`
(`build()`, `build_canonical()`, `marshal()`, `unmarshal()`), but two misuse paths **panic**:
`DoTryBuilder::do_finally()` on a second call (`fn do_finally`) and
`DoCatchBuilder::disposition(ExceptionDisposition::Continued)` (`fn disposition`). Both panics
are intentional, documented, and covered by `#[should_panic]` tests; neither is reachable
from the `Result`-returning terminal paths. This asymmetry is a recorded finding
(camel-builder audit I1) whose resolution is deferred to the code stream — this document
records the **current state**, not the fix direction.

**`RouteBuilder` is `Clone` (rc-8m5o, resolved before v1.0).**
A partially-built route can be cloned and reused as a template for multiple routes,
mirroring Apache Camel's cloneable `RouteBuilder` (ADR-0046 inspiration). Clone deep-copies
the `Vec<BuilderStep>`; step closures already live behind `Arc` (`FilterPredicate`,
`SplitExpression`) or `BoxCloneService` (`OpaqueProcessor`), so a clone shares the closure and
duplicates only the light wrapper. The capability was landed pre-1.0 specifically to avoid
`#[non_exhaustive]`-plus-non-`Clone` entrenchment on the camel-core `BuilderStep` enum (ADR-0049).

**Thread-safety.**
The builder itself is not `Send`/`Sync`-required — it lives only on the construction thread.
Closures pushed into steps (e.g. `filter`, `process`) require `Send + Sync + 'static`, which
is correct for the downstream Tower runtime.

**Not a runtime EIP crate (L7 N/A).**
camel-builder *constructs* route specs; it implements no EIP runtime behavior (that lives in
camel-processor). `build()` / `build_canonical()` are constructors, not pipeline steps, so the
behavioral-parity gate (ADR-0046) does not apply here.

## Related decisions

- **ADR-0001** — Tower data plane / custom-trait control plane. camel-builder constructs the
  data-plane spec (`RouteDefinition` / `CanonicalRouteSpec`); it never touches the control
  plane.
- **ADR-0011 / ADR-0016** — CanonicalRouteSpec v1/v2 minimal contract. `build_canonical()`
  emits v2 with strict rejection of unsupported steps.
- **ADR-0049** — Workspace `#[non_exhaustive]` policy. **N/A by category** here — 0 `pub
  enum`s; cross-referenced only, not extended (see posture table).
- **ADR-0045** — camel-core architecture charter; cross-referenced because `BuilderStep` and
  `RouteDefinition` are camel-core-owned types this crate writes to.
- **camel-config DP-9** (`0a720767`) / **camel-dsl DP-8** (`98ace84e`) — crate-local
  CONTEXT.md precedents this file follows.
- **Open finding (does not block this doc):** I1 — unify the panic-vs-`Result` policy on
  `do_finally` / `disposition`. Tracked in the code stream; this doc records current state
  either way.
