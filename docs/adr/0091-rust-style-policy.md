# ADR-0091: Rust style policy

- Status: Proposed
- Date: 2026-10-27
- Source: bd `rc-9at09`; gap analysis
  `.opencode/fleet/inbox/reference/google-rust-style-gap-analysis-2026-10-27.md`
- Cross-refs: ADR-0090 (pre-1.0 compatibility), ADR-0075 and ADR-0083
  (sealed artifacts), ADR-0049 (error contract)

## Context

The Google Rust Style Guide (2026-10) is the most comprehensive public Rust
style document. We do not adopt it wholesale. Three constraints conflict
with parts of it:

- rust-camel is an async-core framework. `Arc`, `async`, and `.await` are
  load-bearing. Several Google rules target Chrome-only code.
- ADR-0090 says break, do not deprecate, before 1.0. Rules that assume a
  stable public contract do not fit.
- The toolchain is stable 1.89. Nightly-only rustfmt options cannot be
  enforced in CI.

We therefore adopt a subset. Each adopted rule has an enforcement point or
a named review norm. Each rejected rule has a stated reason, not a silent
omission.

The gap analysis body has three errors. Its ERRATA corrects two, and
landing verification corrected the third:

- `hex_literal_case = Lower` is accepted by the `rustfmt --config` CLI but
  **rejected by `cargo fmt`** (the rustfmt-as-a-library path that CI uses)
  as unstable. It cannot be enforced in CI and is **not adopted**.
- `format_macro_matchers` is not available on stable.
- Raw unsafe counts are dominated by test code (90 `unsafe {` blocks, 72
  `SAFETY:` comments).

This ADR follows the verified behaviour, not the report body.

## Decision

### Adopted and enforced

1. **rustfmt is the single formatter.** Scope: whole workspace.
   `rustfmt.toml` at the repo root pins `edition = "2024"` and
   `max_width = 100` (explicit). Do not add knobs that stable `cargo fmt`
   rejects or ignores (`hex_literal_case`, `format_macro_matchers`,
   `imports_granularity`, `group_imports`). Enforcement: the existing
   `cargo fmt --check --all` gate. The tree already matched the stable
   defaults, so adding the file changed no source; re-evaluate
   `hex_literal_case` only when cargo-fmt accepts it as stable.

2. **No glob imports.** Scope: whole workspace, outside test scope.
   Forbid `use ...::*;` except these cases:
   - `use super::*` inside an inline test module;
   - `pub use ...::*` in a non-leaf re-export module;
   - `use enum::*` inside an impl block of that enum;
   - a third-party prelude approved at review, for example
     `tracing_subscriber::prelude::*` or `tower_lsp::lsp_types::*`.

   Enforcement: new `cargo xtask lint-glob-imports`. The adoption PR fixes
   the 24 existing violations.

3. **Prefer `expect("reason")` over `unwrap()`.** Scope: whole workspace.
   Use `expect` when the panic is an invariant violation. The existing
   `cargo xtask lint-unwrap` already enforces this. No extension is needed.
   Existing `unwrap()` sites stay grandfathered under the 1,487
   `// allow-unwrap` escapes.

4. **Every `unsafe` item carries a `SAFETY:` comment.** Scope: whole
   workspace. An `unsafe fn` takes a preceding comment. An `unsafe { }`
   block takes a preceding or initial interior comment. A nested unsafe
   block needs its own justification. Enforcement: new
   `cargo xtask lint-unsafe-safety`. The adoption PR fixes the roughly 18
   missing sites.

5. **`missing_docs` ratchet on the public API crates.** Scope:
   `camel-api`, `camel-core`, `camel-builder`, `camel-processor`,
   `camel-endpoint`, `camel-dsl`. Add `#![warn(missing_docs)]` per crate
   where it is safe. Store a per-crate diagnostic baseline in xtask. CI
   fails on a new diagnostic, not on the absolute count. Do not mass-write
   docs in this adoption. Enforcement: new
   `cargo xtask lint-missing-docs-ratchet`, plus the existing `doc-build`
   gate.

6. **`#[must_use]` on public error types and pure functions.** Scope:
   public error types and pure functions where ignoring the return value is
   a bug. Apply targeted fixes from a one-time `clippy::must_use_candidate`
   review. Do not enable the lint warn-by-default. Enforcement: code
   review.

### Adopted as review norms (no lint)

- One module per `use` statement. Do not nest braces:
  `use foo::{Bar, baz::Baz}`.
- Scope a trait import locally unless the trait is used file-wide.
- Qualify a top-level function call with its module: `mem::transmute(x)`,
  not `transmute(x)`.
- Import type names unqualified. Reach functions through their module.
- Use an `enum` over `bool` for a non-obvious API parameter.
- Use named fields over a multi-field tuple.
- Take iterators or `impl IntoIterator` for a reusable library input.
  Prefer `&[T]`, `&str`, and `&T` over `&Vec`, `&String`, and `&Box`.
- Use `format!` and `write!` over `+` concatenation.
- Pattern-match over check-then-unwrap.
- Add a context string to `assert!` when the failure is not self-evident.
- Do not build a long iterator combinator chain when a `for` loop is
  clearer.
- Name getters and setters `foo()`, `foo_mut()`, `set_foo()`, `take_foo()`,
  and `with_foo()`.
- Keep generic parameters short and evocative. Keep lifetimes short.
- Prefer a function, then `macro_rules!`, then a proc-macro.
- An options struct implements `Default`. Use a newtype over an alias when
  the alias would leak an invariant.

### Explicit exceptions

- **Avoid `Rc` and `Arc`: rejected.** `Arc` is the ownership primitive for
  `Exchange`, processors, and routes, with 7,612 sites.
- **No `async` and no `.await`: rejected.** Chrome-only rule.
- **No `std::thread`: rejected.** Chrome-only rule.
- **`fuchsia_sync::Mutex`: rejected.** Fuchsia-only rule.
- **Flat file layout and small crates: rejected.** The rule exists for
  Chrome monorepo large-scale changes.
- **No `catch_unwind`: rejected.** `camel-language-js` uses it for worker
  self-heal, and `camel-bridge` uses it for the watchdog.
- **`format_macro_matchers`, `imports_granularity`, `group_imports`:
  rejected for now.** They are unstable on rustfmt 1.9.0-stable and MSRV
  1.89.
- **`#[non_exhaustive]` on public structs: rejected for now.** ADR-0090
  makes it less load-bearing before 1.0. `lint-non-exhaustive` continues
  to govern enums.
- **"Avoid abbreviations" as written: rejected.** Idiomatic Rust
  abbreviations (`tx`, `rx`, `ctx`, `db`) are universal in the async
  ecosystem. Narrower norm: do not invent a project-specific initialism
  that a newcomer cannot guess.

### Relationship to existing policy

- ADR-0090 is unchanged. This ADR introduces no deprecation mechanism.
- ADR-0075 and ADR-0083 are unchanged.
- Every existing `cargo xtask lint-*` gate continues to fire. This ADR adds
  two gates (`lint-glob-imports`, `lint-unsafe-safety`) and one ratchet
  (`lint-missing-docs-ratchet`).
- The language policy and the docs governance rules are unaffected.
