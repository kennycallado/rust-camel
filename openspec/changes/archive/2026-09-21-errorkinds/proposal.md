# Proposal: errorkinds

## Why

bd rc-5u8co: static walk of `CamelError` (25 variants,
`crates/camel-api/src/error.rs`) against `supported_exception_kinds()`
(`crates/camel-dsl/src/compile.rs`, 19 entries) shows 6 variants that
`on_exceptions` cannot selectively catch: `ProcessorErrorWithSource`,
`ConfigValidation`, `TemplateReload`, `EndpointUri`, `UnsupportedMediaType`,
`NotAcceptable`. Audit classification: three are raised in-pipeline and only
catchable via the `*` wildcard or fragile `message_contains` (same gap
class as rc-fu1of (f59489d0) and rc-2vm2y (24ba1ee9)); the other three are
startup-only or deferred (see table).

## Audit result (evidence-based classification)

| Variant | Raised | Reaches route error handler? | Verdict |
|---|---|---|---|
| `UnsupportedMediaType` | in-pipeline media negotiation gate (`camel-dsl/src/media.rs:209`, `camel-processor/src/content_negotiation.rs:144`) | YES — flows to `pipeline_error_to_reply` (415) | **should match** |
| `NotAcceptable` | in-pipeline media gate (`media.rs:241`) | YES (406) | **should match** |
| `ProcessorErrorWithSource` | in-pipeline producers (bean, exec, surrealdb) | YES | **should match** |
| `TemplateReload` | lifecycle reload command + endpoint start (`runtime_bus.rs`, `template_reload.rs`) | mostly NO (bypasses handler) | file bd |
| `ConfigValidation` | startup validation / step resolution | NO (fail-fast before routes run) | correctly unmatchable — document |
| `EndpointUri` | YAML parse (`yaml.rs:811`), endpoint build | NO | correctly unmatchable — document |

Stale-bd finding: rc-5u8co's note "'Stopped' is in vocabulary but not a
variant" no longer holds — the `Stopped` arm was removed (ADR-0024 +
directive 2026-06-20); current vocabulary has zero dead entries.

## What changes

1. Register `UnsupportedMediaType`, `NotAcceptable`,
   `ProcessorErrorWithSource` in `supported_exception_kinds()` +
   `exception_kind_matches()`, mirroring the 24ba1ee9 pattern. PEWS gets its
   OWN kind (structural matching, distinct from the `ProcessorError`
   `variant_name()` alias) per the rc-2vm2y alias-distinction precedent.
2. Guard-test hardening: pin bidirectional consistency between the
   vocabulary and a per-variant classification table (matchable /
   intentionally unmatchable / deferred), instead of relying solely on the
   pinned literal list; freshness for newly added variants stays a
   reviewed manual step anchored by the camel-api exhaustive
   `variant_name` test.
3. Document `ConfigValidation` and `EndpointUri` as intentionally
   unmatchable (startup fail-fast) in the vocabulary's doc context.
4. File bds for the controversial remainder: `TemplateReload`
   (handler-reach ambiguity), `EndpointUri`-vs-`InvalidUri` vocabulary
   consistency.

## Acceptance criteria

- `on_exceptions: [{kind: "UnsupportedMediaType"}]` (and the other two)
  compiles, matches its variant, and does not match unrelated variants.
- `kind: "ProcessorError"` does NOT catch `ProcessorErrorWithSource`
  (alias-distinction pin, mirroring 24ba1ee9).
- Guard test pins the current inventory: every enumerated variant is
  classified (registered, intentionally unmatchable, or deferred), keeping
  the register-or-document decision a reviewed manual step — cross-crate
  tables cannot mechanically force classification of future variants.
- Unknown-kind rejection (`ConfigValidation`, `TemplateReload`,
  `EndpointUri`) still errors with the supported-kinds list.

## Risk budget

Additive vocabulary + match arms only; no change to existing kind
semantics, policy ordering, or rendering tables. Low risk. No new public
API surface in camel-api.
