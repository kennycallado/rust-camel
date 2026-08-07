# Proposal: audit-fix-secret-leak-lint

## Why

ADR-0051 deferred mechanical enforcement of credential Debug/Serialize rules
because field-name heuristics produce false positives (`client_key_path`,
cancellation tokens) and false negatives (`StateStore.data`). Change A1
fixed five confirmed violations manually (across six type declarations —
`TokenResponse` appears in two modules). Without enforcement, regression
is inevitable — a new struct with `#[derive(Debug)]` carrying credentials
passes all existing lints and CI gates.

bd issue rc-vh2l tracks this gap. ADR-0051 § Enforcement explicitly defers
to "the T2 audit sweep, or when another confirmed secret-bearing derive
appears." Both conditions are now met.

## What Changes

**Included:**

- Extend `cargo xtask lint-secrets` with AST-based derive inspection using
  `syn` (already a workspace dependency).
- Introduce a closed-vocabulary doc attribute for semantic classification:
  `/// ADR-0051 credential boundary: <classification>` where classification
  is one of: `manual-redaction`, `redacting-wrapper`, `protocol-dto`.
- The lint enforces derive consistency: a type marked `manual-redaction`
  must NOT derive `Debug` or `Serialize`; a type marked `protocol-dto` may
  derive `Serialize` but not `Debug`; a type marked `redacting-wrapper` may
  derive `Debug` but not `Serialize`.
- Annotate all credential-bearing types with their classifications:
  the five A1-fixed types (`TokenResponse` ×2, `StateStore`, `OtelConfig`,
  `BridgeProcessConfig`) as `manual-redaction`, `KafkaConfig` +
  `KafkaBrokerConfig` as `redacting-wrapper` (delegates to nested redacting
  Debug), and nine `Zeroizing<String>`-carrying types in camel-auth as
  `manual-redaction`.
- Enforce closed vocabulary: unknown, malformed, or conflicting duplicate
  classifications produce violations.
- Amend ADR-0051 § Enforcement to replace the deferral text with the
  implemented lint description.
- Unit tests covering: manual-redaction violation, redacting-wrapper safe
  Debug, protocol-dto Serialize exception, `Zeroizing<T>` auto-detection
  (including `zeroize::Zeroizing<T>` qualified path), multiline derives,
  unknown classification rejection, parse-failure hard-fail.

**Excluded:**

- Discovery of credential-bearing types by content inspection (not
  statically detectable; code review owns classification).
- A new xtask subcommand (the lint extends `lint-secrets`).
- Changes to the `// allow-secret` escape hatch (sink lint unchanged).

## Acceptance criteria

- `cargo xtask lint-secrets` exits non-zero when a `manual-redaction` type
  derives `Debug` or `Serialize`.
- All 14 annotated type definitions pass the lint with correct
  classifications (12 `manual-redaction` + 2 `redacting-wrapper`).
- Zero false positives on types like `client_key_path` (metadata, not
  credential-capable) and cancellation tokens.
- ADR-0051 § Enforcement describes the implemented lint, not a deferral.
- AGENTS.md quality gate `lint-secrets` remains green on the full workspace.

## Risk budget

- **Acceptable:** Retroactive doc annotation of 14 existing type definitions
  (trivial, non-breaking).
- **Acceptable:** Adding `syn` to `scripts/xtask/Cargo.toml` (already in
  workspace, zero new external dependency).
- **Out of bounds:** Modifying any public API surface. The lint is build
  tooling only; production code changes are limited to doc comments.
