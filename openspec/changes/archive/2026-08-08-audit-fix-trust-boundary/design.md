# Design: audit-fix-trust-boundary

## Approach

Two independent validation gates, one per affected crate, plus a verification pass
on the already-fixed SQL crate. No shared abstraction — the gates are
semantically different (query-text opt-in vs path-injection validation).

### SurrealDB: `allow_dynamic_query` gate

Mirror the SQL component's pattern:

1. Add `allow_dynamic_query: bool` field to `SurrealDbEndpointConfig` (default `false`).
2. Parse it from URI params in `from_uri`.
3. In `resolve_query_source`, wrap the header/body branches in a conditional
   block gated on `self.config.allow_dynamic_query`. When `false`, only the
   config query is used.
4. When `query` op is selected, `allow_dynamic_query=false`, and no config query is
   set, the producer returns a runtime `MissingParam` error when an exchange arrives
   (not at startup time — the query source is only resolved at invocation).
5. Remove the `warn!` calls on the header/body branches — the gate replaces the
   advisory warning with enforcement.

### OpenSearch: `validate_doc_id` and `resolve_doc_id` validation

1. Add `pub(crate) fn validate_doc_id(id: &str) -> Result<(), ProducerError>` in the producer module.
2. Reject: empty string, null bytes (`\0`), URL-path-significant characters
   (`/`, `?`, `#`, `%`), exact dot segments (`.` and `..`), backslashes (`\`),
   control characters (C0 controls U+0000–U+001F and DEL U+007F, checked via
   `(c as u32) <= 0x1F || c == '\u{7F}'`), and strings exceeding 512 bytes.
3. Returns `ProducerError::Permanent` on validation failure (not Transient —
   validation errors are not retryable).
4. Add `pub(crate) fn resolve_doc_id<'a>(exchange: &'a Exchange) -> Result<Option<&'a str>, ProducerError>`
   that extracts the header and validates it. This is the shared extraction+validation
   point — all `execute_*` functions call it instead of inline header extraction.
5. Add `pub(crate) fn resolve_required_doc_id<'a>(exchange: &'a Exchange, op_name: &str) -> Result<&'a str, ProducerError>`
   for GET/DELETE/UPDATE/EXISTS where the header is mandatory. Returns Permanent error
   when absent.
6. Wire `resolve_doc_id` into `execute_index` (optional doc_id) and
   `resolve_required_doc_id` into `execute_get`, `execute_delete`, `execute_update`,
   `execute_exists`. Each replaces the inline header extraction call.

### SQL: verification

Confirm existing tests cover the `allow_dynamic_query=false` default and the
opt-in path. The real URI param name is `allowDynamicQuery`. Close rc-qek5 as
already-fixed.

## Affected crates

- `camel-component-surrealdb` — config field, producer gate, tests
- `camel-opensearch` — validation function, 5 call sites, tests
- `camel-sql` — verification only (no code changes)

## Dependencies

- ADR-0032 (Exchange-Data Trust Boundary) — the policy this change enforces
- ADR-0033 (Security Defaults) — the fail-closed validation arm
- SQL component's `allow_dynamic_query` pattern — the reference implementation

## Open questions

None. The fix shapes are well-defined by the existing SQL pattern and the
OpenSearch index_name validation precedent.

## Phases

### Phase 1: Trust-boundary gate and validation

- **Goal:** Enforce ADR-0032 in camel-component-surrealdb (query gate),
  camel-opensearch (doc_id validation), and camel-sql (verification).
- **Dependencies:** ADR-0032, ADR-0033, SQL `allow_dynamic_query` reference pattern.
- **Deliverable:** All three crates pass their lib tests; SurrealDB rejects
  untrusted query text by default; OpenSearch rejects path-injection doc IDs.
- **Externally visible interfaces:** New `allow_dynamic_query` config field on
  `SurrealDbEndpointConfig`.
- **Exit criteria:** `cargo test -p camel-component-surrealdb --lib`,
  `cargo test -p camel-opensearch --lib`, `cargo test -p camel-sql --lib` all pass
  with 0 failures.
