# Proposal: audit-fix-trust-boundary

## Why

ADR-0032 establishes that exchange data (headers, body) is untrusted, adversary-controlled.
Three components route untrusted exchange data into interpretable sinks without proper
gates:

1. **camel-component-surrealdb** (`query` op): The `CamelSurrealDbQuery` header and
   exchange body flow directly into `run_raw_query` with only a `warn!` log. No
   `allow_dynamic_query` opt-in exists. An attacker on an untrusted route can inject
   arbitrary SurrealQL (incl. DDL, RBAC).

2. **camel-opensearch** (5 operations): The `CamelOpenSearch.Id` header flows unvalidated
   into `_doc/{id}` request builders (GET, DELETE, UPDATE, EXISTS, explicit-ID INDEX).
   Path traversal characters (`/`, `?`, `#`, `%`, null bytes, dot segments, backslashes,
   control characters) can manipulate the request URL path.

3. **camel-sql** (`query` op): Already fixed — has `allow_dynamic_query=false` default
   gate. Included for verification and issue closure.

## What Changes

- **SurrealDB:** Add `allow_dynamic_query: bool` config field (default `false`). Gate
  `resolve_query_source` behind it: when `false`, header and body query text is ignored
  (only config query used). If no config query is set, the producer returns a runtime
  `MissingParam` error at invocation time (not startup time — the query text source is
  only resolved when an exchange arrives). Mirror the SQL component's pattern exactly.
- **OpenSearch:** Add `validate_doc_id(id: &str) -> Result<(), ProducerError>` function.
  Reject null bytes, URL-path-significant characters (`/`, `?`, `#`, `%`), exact dot
  segments (`.` and `..`), backslashes (`\`), control characters (U+0000–U+001F,
  U+007F), empty strings, and strings > 512 bytes. Returns `ProducerError::Permanent`
  on validation failure (not retryable). Apply at all 5 call sites where
  `CamelOpenSearch.Id` header is read: INDEX (explicit-ID path), GET, DELETE, UPDATE,
  EXISTS.
- **SQL:** Verification only — existing gate confirmed. Close rc-qek5.

## Acceptance criteria

- SurrealDB `query` op ignores header/body query text when `allow_dynamic_query=false`
- SurrealDB `query` op accepts header/body query text when `allow_dynamic_query=true`
- SurrealDB `query` op with `allow_dynamic_query=false` and no config query returns
  runtime MissingParam error
- OpenSearch rejects doc_id containing `/`, `?`, `#`, `%`, `\`, `.`, `..`, control
  characters, null bytes, empty, or > 512 bytes
- OpenSearch validation applies to all 5 operations (INDEX, GET, DELETE, UPDATE, EXISTS)
- All existing tests pass
- ADR-0032 compliance: no untrusted exchange datum reaches an interpretable sink without
  explicit operator opt-in or validation

## Risk budget

**Behavioral changes (migration required):**
- SurrealDB: existing routes that relied on header/body query text without
  `allow_dynamic_query=true` will stop working. Operators must add
  `allow_dynamic_query=true` to restore the behavior. This is the correct ADR-0032
  posture — the previous behavior was a security gap, not an accepted contract.
- OpenSearch: existing routes where `CamelOpenSearch.Id` contains newly forbidden
  characters (e.g. `.` standalone, `..`, `\`, control chars) or exceeds 512 bytes will
  now fail with a Permanent error. Operators with such IDs must sanitize upstream or
  use different ID conventions.

Low implementation risk. Both fixes are additive validation gates that fail closed.
No public API contracts change (new config field defaults to safe).
