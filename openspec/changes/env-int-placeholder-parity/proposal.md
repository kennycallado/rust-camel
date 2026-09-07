# Proposal: env-int-placeholder-parity

**Bd:** rc-93wct (bug, p2) · **Related:** rc-ayke (comment placeholders, NOT blocked on), rc-gykds (openapi latent, out of scope)

> **Rev 2 (2026-09-07, wave-E collision — bd rc-93wct, e_glm verdict D).** The "Why" premise is superseded: since wave E, discovery's YAML arm tree-walks and substituted leaves KEEP string typing — the real boot path now REJECTS integer-field placeholders. Rev 2 inverts the goal: loader, LEAN, and lint align to that canon (int-position fails everywhere, boot parity; string-position is the happy path). The Acceptance Criteria bullets about integer fields passing lint/LEAN are superseded by the rev-2 delta specs.

## Why

`${env:NAME:-default}` placeholders in integer-typed DSL fields (u64/usize — e.g. `throttle.max_requests`, `circuit_breaker.open_duration_ms`) work on the real boot path (`camel run` → discovery interpolates raw text before YAML parse, so YAML re-infers the integer) but fail on two surfaces that inspect the raw, unsubstituted text:

1. `camel lint` R-SCHEMA validates the raw document against ROUTE_SCHEMA; a placeholder string where `"type": "integer"` is expected is a type violation.
2. The LEAN unit test tier (`camel test --unit`) loads route files via `camel_dsl::load_from_file` and inline `routes:` via `parse_yaml` — neither interpolates, so serde fails on the literal placeholder string and the whole test document errors.

Impact (demo team): 5 production routes parametrizing `throttle.max_requests` are excluded from the CI lint job; `circuit_breaker.open_duration_ms` cannot be shortened for tests (CB recovery timing untestable at ~60 s). LSP shows the same false error live (shares the lint engine).

## What Changes

Align the broken consumers with the discovery.rs interpolation contract, using a **default-only lookup** (`interpolate_env_with(src, &|_| None)` — never ambient process env, preserving LEAN determinism per ADR-0069 §13.1):

- **camel-dsl**: `load_from_file` interpolates with the default-only lookup; new injectable `load_from_file_with_env(path, lookup)`. Unset-no-default → `Err` naming the variable (boot-parity wording).
- **camel-cli** (test runner): both file forms (`routeFiles`, `routeFilesFromRoot`) and the inline-`routes` branch interpolate default-only; fix the false parity doc comment; unresolved-no-default surfaces as a `doc_error` mirroring discovery's wording.
- **camel-lint**: R-SCHEMA validates an interpolated copy (default-only; a no-default token is left literal so the schema error still surfaces); emits one non-failing `Severity::Info` note per substituted default. The interpolator is duplicated into camel-lint (crate purity forbids a camel-dsl dep) with a `// SYNC:` mirror annotation. LSP inherits both behaviors for free (shares the engine).

## Acceptance Criteria

- `camel lint` passes (exit 0) on an integer field with `${env:X:-2}`; one Info note reports the substituted default.
- LEAN unit docs load routes (file and inline) carrying integer placeholders; CB `open_duration_ms: ${env:CB_MS:-750}` boots with 750 ms.
- No-new-code reads `std::env` in lint or LEAN load paths (determinism: diagnostics are a pure function of buffer/file text).
- Corpus baseline updated; no false positives.

## Risk Budget

Bounded widening of rc-ayke: commented no-default `${env:X}` becomes a diagnosable LEAN/lint error (parity with already-shipped boot behavior). Guard test: commented with-default stays harmless. Out of scope: rc-ayke itself, rc-gykds, StringOrInt/schema changes.

**Affected crates:** camel-dsl, camel-cli, camel-lint.
