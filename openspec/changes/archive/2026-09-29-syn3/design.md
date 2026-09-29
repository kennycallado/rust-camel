# Design: syn3

## Approach

Compile-driven migration, one flag flip then per-crate repair — with the flag
flip gated on captured baselines. The syn 3.0.0 release notes enumerate the
breaking changes; against our actual call sites the pre-declared break surface
is at most one site (`Receiver.reference`); everything else is
compile-discovered. Workspace pin moves `syn = "2.0"` -> `syn = "3"` once
(single-source rule), then each consumer is migrated against its own compile
errors and its own test battery.

### Behavior-neutrality proof (pre/post pin byte comparison)

The parity oracle is a captured-before / compared-after byte diff, NOT the DSL
goldens (goldens lock serialized route config/warnings/errors, not macro
expansions — they stay as downstream regression coverage):

1. **Macro expansion baselines (syn 2, before the pin flip).** Each macro crate
   gains a byte-snapshot test following the existing internal-fn unit-test
   pattern (`bean-macros/src/lib.rs:252+` already parses `ItemImpl` via
   `parse_quote!` and calls the generator directly): representative inputs for
   `bean_impl!` (ok case, generic-impl reject, no-handler reject), `#[handler]`
   (`&self` ok, `self` reject, non-async reject, duplicate-param reject), and
   `#[derive(UriConfig)]` (ok struct, option-kind inference, error attrs) are
   run through the generator and its `syn::Error` diagnostics; the exact
   `TokenStream::to_string()` / error-message bytes are written to committed
   baseline files. Captured on syn 2, then the pin flips; after migration the
   same tests must pass byte-identical.
2. **xtask lint verdict surfaces.** Before the flip, run the syn-consuming
   lints (`lint-test-sleep`, `lint-metric-labels`, `lint-unbounded-wait`,
   `lint-context-citations` — which also parses Rust with syn — plus the
   syn-based collectors in `main.rs`) over the unchanged workspace tree and
   capture sorted finding tuples (path, line, message) + exit codes + ratchet
   counts; after migration, re-run and byte-compare. Ratchet `.max` files must
   stay byte-identical.
3. **Lock audit.** `Cargo.lock` edge/package diff against the pre-migration
   lock; every changed entry must be justified as the syn edge move (syn 3.0.4
   already in lock via `async-trait 0.1.92`; syn 2.0.119 remains for
   unmigrated third-party proc-macros — expected, not churn).

### syn 2 -> 3 mapping vs our usage

Inventory sources: `rg 'syn::'` over the three consumers (7 files import syn)
plus the PR #40 release notes.

| syn 3.0 breaking change | Our call sites | Verdict |
| --- | --- | --- |
| `Receiver` fields split into non-exhaustive `ReceiverKind` | `crates/camel-bean-macros/src/handler.rs:84` reads `receiver.reference.is_some()` to enforce `&self` handlers | SUSPECTED REWRITE — adapt to syn 3 `Receiver` shape, keep the `&self`-only policy byte-for-byte in diagnostics; compile confirms |
| `Local::init: Option<LocalInit>` (`expr` + `diverge`) | `scripts/xtask/src/lint_unbounded_wait.rs:1705,1810,2210` already reach `.init.expr` (the `LocalInit` shape landed mid-syn-2) | NO CHANGE EXPECTED — compile confirms; any residual delta is compile-discovered, not pre-declared |
| 10 new `*Modifiers` structs; flags regrouped | none constructed or destructured; `ItemFn`/`ImplItemFn`/`TraitItemFn` touched only via `.attrs`, `.sig` | no impact expected; compile check |
| `Signature.unsafety` -> `Safety` enum | only `.sig.ident`, `.sig.inputs`, `.sig.asyncness`, `.sig.output` | none |
| `Arm.guard` -> `Pat::Guard` inside arm pat | no `.guard` reads | none |
| `ExprClosure` `or1/or2_token` -> `inputs_begin/inputs_end` | only `c.inputs` (lint_test_sleep.rs:289) | none |
| `Type::BareFn` -> `Type::FnPtr`; `Type::Ptr` mutability enum; attrs on all `Type` variants | none of these variants matched | none |
| `GenericParam::Type/Const` default shape; `WherePredicate` attrs | only `item.generics.params.is_empty()` (bean lib.rs:68) | none |
| `Punctuated::pop` -> `Option<T>` | `.pop()` only on `Vec` (xtask stacks) | none |
| Some `From` impls removed (e.g. into `Expr`) | no `.into()` into syn enums | none |
| `visit`/`fold` no longer visit `Span` | visitors never override span walking | none |
| `File` gains `Option<Frontmatter>` | `parse_file` callers never construct `File` literals | none |
| attrs now preserved on all expr-statement kinds | read-only visitors; strictly more fidelity, no verdict logic keys on absence of stmt attrs | benign; watch xtask ratchet counts |

Stable API in heavy use (unchanged in syn 3): `parse_macro_input`, `ParseStream`,
`Token![...]`, `parenthesized!`, `parse_quote!`, `parse_file`, `Meta::{Path,List,
NameValue}`, `attr.meta.require_list()`, `Expr::Lit`, `LitStr`, `ExprArray`,
`PathArguments::AngleBracketed`, `GenericArgument::Type`, `Fields`, `UseTree::*`,
`spanned::Spanned`, the `visit_*` callback set we override. Feature names
`full`/`parsing`/`extra-traits`/`visit` all still exist in syn 3.

Lock: syn 3.0.4 is already in `Cargo.lock` via `async-trait 0.1.92`, so the bump
mostly rewires our three crates' edges onto the existing 3.0.4 node. syn 2.0.119
stays in the lock for unmigrated transitive proc-macros (serde_derive et al.) —
that is expected and is not churn. Diff audit gates on: only syn-package
adjacent edges change.

## Affected crates

- `Cargo.toml` (workspace root): `syn = "2.0"` -> `syn = "3"`.
- `crates/camel-bean-macros`: `handler.rs` Receiver rework; `lib.rs` compile
  pass; lib test battery (parse_quote-driven, 18 existing cases + 6 expansion
  baselines).
- `crates/camel-endpoint-macros`: `lib.rs` + `uri_config.rs` expected
  compile-clean (stable API only); unit battery (trybuild declared, no suite —
  noted, skipped).
- `scripts/xtask`: expected compile-clean (`LocalInit` shape already in use);
  full xtask suite (667+ src tests + 4 `archive_e2e.rs` tests) + pre/post lint
  verdict capture via the built binary (`./target/debug/xtask <lint>`, never
  `cargo xtask` — cargo's compile/`Finished` noise is not byte-stable).
- Downstream regression: `camel-dsl --lib` (854 tests) + `tests/goldens` corpus;
  trybuild is a declared dev-dep in camel-endpoint-macros but no suite exists in
  `tests/` — noted, skipped.

## Architecture boundaries

Proc-macro layer internals only. Zero public-API change: `#[derive(UriConfig)]`,
`#[handler]`, `bean_impl!` emit identical tokens (proven by the pre/post
expansion baselines above); xtask lint verdict surfaces unchanged (proven by
pre/post finding-tuple capture). The bean/endpoint proc-macros are their own
boundary — distinct from the YAML/JSON route DSL per CONTEXT-MAP.md — so
`camel-bean`/`camel-endpoint` re-export compilation and the `camel-dsl` battery
(854 lib tests, goldens) run as downstream regression coverage, not as the
token oracle. No Runtime/Components/Services/Languages code is touched.

## Alternatives considered

- Dual-pin (`syn2` + `syn3` aliases) for staged migration — rejected: violates
  house single-source rule for workspace deps; unnecessary at this break size.
- Wait for ecosystem full-syn3 then bump — rejected: mission order requires the
  migration now; async-trait already proves syn 3 works in this graph.
- cargo-expand byte-diff as parity oracle — rejected: binary unavailable
  offline; the pre/post expansion baselines above are the token oracle, and
  goldens + DSL battery stay as supplementary downstream regression coverage.

## Phases

Single-phase change — coherent dependency slice, no milestone grouping.
