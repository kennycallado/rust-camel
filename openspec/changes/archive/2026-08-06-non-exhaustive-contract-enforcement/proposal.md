# Proposal: non-exhaustive-contract-enforcement

## Why

ADR-0049 (commit ec65888d) makes `#[non_exhaustive]` the default for contract
enums in the three public contract crates (`camel-api`, `camel-component-api`,
`camel-language-api`), but execution is at ~5%: only 3 of 56 `pub enum`s in those
crates carry the attribute, and there is no CI gate preventing regression. If the
v1.0 API freeze passes with closed enums, ADR-0049 is dead-lettered, and adding
the attribute post-freeze is itself a breaking change. This change executes
ADR-0049's code stream (rc-3pw3) AND adds the enforcement lint (rc-ierl) so the
policy cannot silently regress.

## What Changes

**Included:**

- Apply `#[non_exhaustive]` to every `pub enum` in the three contract crates
  that is not a deliberate closed-set exception (ADR-0049 §Exceptions).
- Add an `exhaustive-by-contract` rustdoc note to the two deliberate
  exceptions: `PipelineOutcome` (ADR-0024 outcome algebra) and `ExchangePattern`
  (fixed MEP dichotomy).
- Fix every out-of-crate `match` site broken by the new attributes (add `_ =>`
  arms with correct semantics).
- Add `cargo xtask lint-non-exhaustive` mirroring the existing
  `lint-unwrap`/`lint-secrets`/`lint-log-levels` pattern.
- Add the new lint to the AGENTS.md QUALITY GATES block.

**Excluded:**

- Structs — ADR-0049 is enum-only by title, scope table, and rationale.
  `#[non_exhaustive]` on structs breaks literal construction out-of-crate, a
  different and larger ergonomic contract. Deferred.
- Authoring/schema crates (`camel-dsl`, `camel-config`, `camel-cli`,
  `camel-builder`) — advisory posture per ADR-0049, not enforced.

## Acceptance criteria

- Every `pub enum` in the three contract crates carries `#[non_exhaustive]` OR
  an `exhaustive-by-contract` note.
- `cargo build --workspace` compiles; all out-of-crate match sites fixed.
- `cargo xtask lint-non-exhaustive` exists and exits 0 on the compliant crates.
- The lint fails when an attribute/note is removed (negative test).
- AGENTS.md QUALITY GATES includes the new step.
- No new clippy warnings; `cargo fmt --check` clean.

## Risk budget

- **Acceptable:** one-time ergonomic cost of `_ =>` arms in out-of-crate match
  sites (intentional — converts future additive variants from breaking to
  non-breaking).
- **Out of bounds:** changing any enum's variant set; touching structs; gating
  authoring/schema crates.

Bd: rc-3pw3, rc-ierl.
