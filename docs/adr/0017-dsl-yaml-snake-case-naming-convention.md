# ADR-0017: DSL YAML Key Naming Convention

**Date:** 2026-06-09
**Status:** Accepted
**Issues:** rc-co1

## Decision

All YAML DSL step keys and field names use **snake_case**. No camelCase, no kebab-case, no PascalCase.

## Rule

1. YAML step keys (`set_header`, `wire_tap`, `poll_enrich`, etc.) are snake_case.
2. YAML field names inside step structs (`timeout_ms`, `cache_ttl_secs`, etc.) are snake_case.
3. `serde(rename = "...")` is only allowed for Rust reserved words (`loop`, `while`, `type`, etc.).
4. EIP pattern names in prose/docs/comments ("pollEnrich", "wireTap") remain as-is — they reference the pattern, not the YAML key.

## Rationale

Before this ADR, `pollEnrich` had a `serde(rename = "pollEnrich")` that forced camelCase for one step key while all ~30 other step keys (`set_header`, `set_body`, `wire_tap`, `stream_cache`, `convert_body_to`, `load_balance`, `dynamic_router`, `routing_slip`, `recipient_list`, etc.) were already snake_case. This was an inconsistency that broke the established pattern.

## Consequences

- The YAML key for poll-enrich is `poll_enrich` (not `pollEnrich`).
- Existing YAML route files using `pollEnrich` must update to `poll_enrich`.
- Future DSL additions follow snake_case by default (Rust field naming already enforces this via serde default behavior).

## Amendment (2026-06-26)

The snake_case rule applies to **both YAML and JSON DSL keys**. The original wording
said "DSL YAML" because YAML was the only authoring format at the time (2026-06-09);
JSON inherited the rule silently via the shared `RouteDsl*` AST
(`crates/camel-dsl/src/route_ast.rs`).

ADR-0026 (JSON Canonical Route Authoring Format) formalizes JSON as a first-class
authoring format. To remove ambiguity, this amendment makes explicit that:

- All DSL keys (step keys AND field names) are snake_case regardless of whether the
  user authors routes in YAML or JSON.
- `serde(rename = "...")` policy (Rule 3) is unchanged.
- EIP pattern names in prose/docs/comments (Rule 4) remain as-is.

No behavior change. Existing JSON route files already follow this rule; the
amendment is a documentation clarification.

**Referenced by:** ADR-0026.
