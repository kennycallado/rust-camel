# Design: non-exhaustive-contract-enforcement

## Approach

Two-phase delivery inside one OpenSpec change, because the lint (Phase 2)
validates the fix (Phase 1) against the same ADR-0049 surface.

**Phase 1 — Apply ADR-0049.** Attribute application across the three contract
crates. The compiler is the source of truth for blast radius: after each
crate's enums gain `#[non_exhaustive]`, `cargo build --workspace` reports every
out-of-crate `match` that lost exhaustiveness; each is repaired with a `_ =>`
arm that is **forward-safe**: `unreachable!()` is permitted ONLY when the arm
is justified by an invariant independent of the current variant set (e.g. a
constructor that guarantees a known variant); every other site MUST use an
explicit error/default branch with behavioural coverage. A bare `unreachable!()`
that assumes "no other variant exists" is exactly the silent-mishandle hole
ADR-0049 exists to prevent and is forbidden. In-crate matches are unaffected
(Rust rule: `#[non_exhaustive]` only forces a wildcard arm outside the defining
crate). The two deliberate closed-set exceptions (`PipelineOutcome` per
ADR-0024, `ExchangePattern`) receive a `/// exhaustive-by-contract: <rationale>`
rustdoc note (non-empty rationale, directly attached to the item) instead of
the attribute (ADR-0049 §Rule 3).

**Phase 2 — Enforcement lint.** A new `cargo xtask lint-non-exhaustive` command
mirrors the established `lint-unwrap`/`lint-secrets`/`lint-log-levels` pattern
(`scripts/xtask/src/main.rs`): a `Commands::LintNonExhaustive` variant, a
`lint_non_exhaustive(workspace_root) -> Result<Vec<Violation>, String>` walker
restricted to the three contract crates, and a `lint_non_exhaustive_src(src) ->
Vec<Violation>` unit-testable core. The lint flags any `pub enum` whose
preceding attribute/comment window lacks BOTH `#[non_exhaustive]` AND an
`exhaustive-by-contract` marker. Added to the AGENTS.md QUALITY GATES block.

## Affected crates

- `camel-api`: 48 `pub enum`s gain `#[non_exhaustive]` (53 total minus 3
  already-compliant `CamelError`/`ConfigValidationError`/`TemplateError` minus
  2 exceptions); 2 gain the exception note (`PipelineOutcome`,
  `ExchangePattern`).
- `camel-component-api`: 2 enums (`ConsumerStartupMode`, `ConcurrencyModel`).
- `camel-language-api`: 1 enum (`LanguageError`).
- Every workspace crate with an out-of-crate `match` on the above:
  compiler-discovered, repaired in place.
- `scripts/xtask`: new lint command + dispatch + unit tests.

## Architecture boundaries

This change touches only the **contract surface** (the `*-api` crates) and
**tooling** (`scripts/xtask`). It does not alter the data/control plane split
(ADR-0001), the CQRS bus (ADR-0002), or the pipeline outcome algebra
(ADR-0024 — `PipelineOutcome` stays exhaustive by design). The attribute
additions are additive to compilation for in-crate consumers and
semver-protective for out-of-crate consumers; no behaviour changes. The lint is
a quality gate, not a runtime component.

## Phases

### Phase 1: Apply ADR-0049 contract-enum attributes

- **Goal:** every `pub enum` in the three contract crates complies with
  ADR-0049 (attribute or documented exception).
- **Dependencies:** ADR-0049 (policy), ADR-0024 (PipelineOutcome exception
  rationale).
- **Externally-visible types/interfaces:** `#[non_exhaustive]` added to 51
  contract enums (48 camel-api + 2 camel-component-api + 1 camel-language-api);
  2 exception rustdoc notes (`PipelineOutcome`, `ExchangePattern`);
  out-of-crate forward-safe match arms added across workspace.
- **Deliverable:** compiling, clippy-clean workspace.
- **Exit-criteria:** `cargo build --workspace` succeeds; `cargo clippy
  --workspace --all-features ... -- -D warnings` clean; every contract-crate
  `pub enum` has `#[non_exhaustive]` or the exception note.

### Phase 2: Enforcement lint

- **Goal:** `cargo xtask lint-non-exhaustive` prevents regression of ADR-0049.
- **Dependencies:** Phase 1 (the lint's passing baseline is the compliant
  crates).
- **Externally-visible types/interfaces:** new `LintNonExhaustive` xtask
  subcommand; new QUALITY GATES entry in AGENTS.md.
- **Deliverable:** xtask command + unit tests + AGENTS.md update.
- **Exit-criteria:** `cargo xtask lint-non-exhaustive` exits 0 on the now-
  compliant crates; a negative unit test (enum missing both attribute and note)
  is flagged; AGENTS.md QUALITY GATES updated.

## Alternatives considered

- **Structs in scope too** (rc-ierl wording says "pub enum/struct"): rejected —
  ADR-0049 is enum-only by title, scope table, and rationale (match-breakage).
  `#[non_exhaustive]` on structs breaks literal construction out-of-crate, a
  different and larger ergonomic contract. Deferred; tracked separately if
  needed.
- **Allowlist lint (only the 13 named enums):** rejected — would not catch
  future contract enums (ADR-0049 Rule 1: new enums non_exhaustive from birth).
  The marker-based escape hatch (`exhaustive-by-contract`) gives every enum a
  classification path without an allowlist to maintain.
- **syn-based AST lint vs regex walk:** regex walk (matching `lint-log-levels`)
  is sufficient; the attribute/note always sits in a fixed preceding-line
  window. Keeps the dependency surface identical to existing lints.
