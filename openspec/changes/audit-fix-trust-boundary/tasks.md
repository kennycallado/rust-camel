# Tasks: audit-fix-trust-boundary

## Phase 1: Trust-boundary gate and validation

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

## Task 1: SurrealDB allow_dynamic_query gate

### Files

- `crates/components/camel-component-surrealdb/src/config.rs` (modified)
- `crates/components/camel-component-surrealdb/src/producer.rs` (modified)

### Steps

1. In `config.rs`, add `pub allow_dynamic_query: bool` field to `SurrealDbEndpointConfig`
   struct (line 72: `#[derive(Clone, Debug)]` struct). Place it after `pub query: Option<String>`
   (line 85). The struct derives `Debug` via `#[derive(Clone, Debug)]` at line 71, so the
   field is automatically included in debug output — no manual Debug impl to update.

2. In `config.rs` `from_uri` (line 131), parse the `allow_dynamic_query` URI param.
   Add parsing after the existing query param parsing block. Accept `"true"` and `"false"`
   (case-insensitive via `to_ascii_lowercase()` comparison). Any other value returns
   `CamelError::InvalidUri("allow_dynamic_query must be 'true' or 'false'".to_string())`.
   Store the parsed bool in the struct at the construction site (line 299, after the
   `retry_set_from_uri` field in the struct literal). Initialize to `false` at the default
   construction site (line 117, after `retry_set_from_uri: false`).

3. In `producer.rs` `resolve_query_source` (line 79), wrap the header branch
   (lines 80-91) and the body branch (lines 93-107) in a conditional block gated on
   `if self.config.allow_dynamic_query`. When `false`, skip both branches and fall
   through to the config query (Priority 3). Remove the `warn!` calls — the gate
   replaces advisory logging with enforcement.

4. In `producer.rs`, update the doc comment on `resolve_query_source` (lines 73-78)
   to document the gate: header/body query text is only used when
   `allow_dynamic_query=true`; otherwise the config query is used exclusively.

5. In `producer.rs`, extract the empty-string check from `execute_query` (lines 230-234)
   into a new `pub(crate)` method `resolve_validated_query(&self, exchange: &Exchange) -> Result<String, SurrealDbError>`
   that calls `resolve_query_source`, checks `if sql.is_empty()`, and returns
   `Err(SurrealDbError::MissingParam("query text (body or CamelSurrealDbQuery header)".into()))`
   on empty. On success returns `Ok(sql)`. Update `execute_query` to call
   `let sql = self.resolve_validated_query(exchange)?;` instead of the inline check.
   This makes the runtime error path directly testable without a SurrealClient.

### Tests

- name: `surrealdb_allow_dynamic_query_defaults_to_false`
  setup: `let config = SurrealDbEndpointConfig::from_uri("surrealdb:query?datasource=mydb").unwrap();`
  action: Access `config.allow_dynamic_query`
  assert: `assert!(!config.allow_dynamic_query)`
  command: `cargo test -p camel-component-surrealdb --lib surrealdb_allow_dynamic_query_defaults_to_false`
  expected: pass after implementation

- name: `surrealdb_allow_dynamic_query_true_parsed`
  setup: `let config = SurrealDbEndpointConfig::from_uri("surrealdb:query?datasource=mydb&allow_dynamic_query=true").unwrap();`
  action: Access `config.allow_dynamic_query`
  assert: `assert!(config.allow_dynamic_query)`
  command: `cargo test -p camel-component-surrealdb --lib surrealdb_allow_dynamic_query_true_parsed`
  expected: pass after implementation

- name: `surrealdb_allow_dynamic_query_invalid_maybe_rejected`
  setup: `let result = SurrealDbEndpointConfig::from_uri("surrealdb:query?datasource=mydb&allow_dynamic_query=maybe");`
  action: Check the `from_uri` Result
  assert: `assert!(result.is_err())`
  command: `cargo test -p camel-component-surrealdb --lib surrealdb_allow_dynamic_query_invalid_maybe_rejected`
  expected: pass after implementation

- name: `surrealdb_allow_dynamic_query_yes_rejected`
  setup: `let result = SurrealDbEndpointConfig::from_uri("surrealdb:query?datasource=mydb&allow_dynamic_query=yes");`
  action: Check the `from_uri` Result
  assert: `assert!(result.is_err())`
  command: `cargo test -p camel-component-surrealdb --lib surrealdb_allow_dynamic_query_yes_rejected`
  expected: pass after implementation

- name: `surrealdb_allow_dynamic_query_one_rejected`
  setup: `let result = SurrealDbEndpointConfig::from_uri("surrealdb:query?datasource=mydb&allow_dynamic_query=1");`
  action: Check the `from_uri` Result
  assert: `assert!(result.is_err())`
  command: `cargo test -p camel-component-surrealdb --lib surrealdb_allow_dynamic_query_one_rejected`
  expected: pass after implementation

- name: `surrealdb_query_rejects_header_by_default`
  setup: Config with `operation=Query`, `query=Some("SELECT * FROM t".to_string())`, `allow_dynamic_query=false`. Exchange with `CamelSurrealDbQuery` header set to `"SELECT * FROM users"`.
  action: Call `producer.resolve_query_source(&exchange)`
  assert: `let result = producer.resolve_query_source(&exchange); assert_eq!(result, "SELECT * FROM t")`
  command: `cargo test -p camel-component-surrealdb --lib surrealdb_query_rejects_header_by_default`
  expected: pass after implementation

- name: `surrealdb_query_accepts_header_with_opt_in`
  setup: Config with `operation=Query`, `query=Some("SELECT * FROM t".to_string())`, `allow_dynamic_query=true`. Exchange with `CamelSurrealDbQuery` header set to `"SELECT * FROM users"`.
  action: Call `producer.resolve_query_source(&exchange)`
  assert: `let result = producer.resolve_query_source(&exchange); assert_eq!(result, "SELECT * FROM users")`
  command: `cargo test -p camel-component-surrealdb --lib surrealdb_query_accepts_header_with_opt_in`
  expected: pass after implementation

- name: `surrealdb_query_rejects_body_text_by_default`
  setup: Config with `operation=Query`, `query=Some("SELECT * FROM t".to_string())`, `allow_dynamic_query=false`. Exchange with `Body::Text("SELECT * FROM users".to_string())` and no header.
  action: Call `producer.resolve_query_source(&exchange)`
  assert: `let result = producer.resolve_query_source(&exchange); assert_eq!(result, "SELECT * FROM t")`
  command: `cargo test -p camel-component-surrealdb --lib surrealdb_query_rejects_body_text_by_default`
  expected: pass after implementation

- name: `surrealdb_query_accepts_body_text_with_opt_in`
  setup: Config with `operation=Query`, `query=Some("SELECT * FROM t".to_string())`, `allow_dynamic_query=true`. Exchange with `Body::Text("INFO FOR DB".to_string())` and no header.
  action: Call `producer.resolve_query_source(&exchange)`
  assert: `let result = producer.resolve_query_source(&exchange); assert_eq!(result, "INFO FOR DB")`
  command: `cargo test -p camel-component-surrealdb --lib surrealdb_query_accepts_body_text_with_opt_in`
  expected: pass after implementation

- name: `surrealdb_query_accepts_body_json_string_with_opt_in`
  setup: Config with `operation=Query`, `allow_dynamic_query=true`, no config query. Exchange with `Body::Json(serde_json::Value::String("INFO FOR DB".into()))` and no header.
  action: Call `producer.resolve_query_source(&exchange)`
  assert: `let result = producer.resolve_query_source(&exchange); assert_eq!(result, "INFO FOR DB")`
  command: `cargo test -p camel-component-surrealdb --lib surrealdb_query_accepts_body_json_string_with_opt_in`
  expected: pass after implementation

- name: `surrealdb_query_no_source_gate_off_returns_runtime_error`
  setup: Config with `operation=Query`, `query=None`, `allow_dynamic_query=false`. Exchange with no query header, empty body.
  action: Call `producer.resolve_validated_query(&exchange)`
  assert: `let result = producer.resolve_validated_query(&exchange); assert!(result.is_err()); let err = result.unwrap_err(); assert!(matches!(err, SurrealDbError::MissingParam(_)))`
  command: `cargo test -p camel-component-surrealdb --lib surrealdb_query_no_source_gate_off_returns_runtime_error`
  expected: pass after implementation

### Acceptance

- `cargo test -p camel-component-surrealdb --lib` passes with 0 failures
- `cargo clippy -p camel-component-surrealdb -- -D warnings` exits 0
- `cargo fmt --check` passes
- Existing test `test_query_via_header_with_empty_body_accepted` (line 1016) is updated to set
  `allow_dynamic_query=true` on the config (it now requires the gate)

- [x] task-1

## Task 2: OpenSearch validate_doc_id validation

### Files

- `crates/components/camel-opensearch/src/producer/mod.rs` (modified)

### Steps

1. Add a `pub(crate)` function `validate_doc_id(id: &str) -> Result<(), ProducerError>`
   in the `impl OpenSearchProducer` block in `producer/mod.rs`. The function rejects:
   - empty string: `id.is_empty()` returns true
   - null bytes: `id.contains('\0')` returns true
   - forward slash: `id.contains('/')` returns true
   - question mark: `id.contains('?')` returns true
   - hash: `id.contains('#')` returns true
   - percent sign: `id.contains('%')` returns true
   - backslash: `id.contains('\\')` returns true
   - exact dot segments: `id == "." || id == ".."` returns true
   - C0 control characters and DEL: `id.chars().any(|c| (c as u32) <= 0x1F || c == '\u{7F}')` returns true
   - length > 512: `id.len() > 512` returns true
   Each rejection returns
   `Err(ProducerError::Permanent(format!("invalid doc_id: {}", reason)))`
   where `reason` is a short string like `"contains path separator"`.
   On success returns `Ok(())`.

2. Add a `pub(crate)` function
   `resolve_doc_id<'a>(exchange: &'a Exchange) -> Result<Option<&'a str>, ProducerError>`
   in the `impl OpenSearchProducer` block. This function extracts the
   `CamelOpenSearch.Id` header value via `exchange.input.header("CamelOpenSearch.Id").and_then(|v| v.as_str())`.
   If `Some(id)`, calls `validate_doc_id(id)?` and returns `Ok(Some(id))`.
   If `None`, returns `Ok(None)`. The explicit lifetime `'a` ties the returned `&str`
   to the exchange borrow.

3. Add a `pub(crate)` function
   `resolve_required_doc_id<'a>(exchange: &'a Exchange, op_name: &str) -> Result<&'a str, ProducerError>`
   in the `impl OpenSearchProducer` block. Calls `Self::resolve_doc_id(exchange)?`.
   If `Some(id)`, returns `Ok(id)`. If `None`, returns
   `Err(ProducerError::Permanent(format!("Missing CamelOpenSearch.Id header for {} operation", op_name)))`.
   The explicit lifetime `'a` ties the returned `&str` to the exchange borrow.

4. In `execute_index` (line 251), replace the inline header extraction at line 258 with
   `let doc_id: Option<&str> = Self::resolve_doc_id(exchange)?;`. The validation is now
   inside `resolve_doc_id`, called before the `match doc_id` at line 263.

5. In `execute_get` (line 314), replace the inline header extraction + None-check at
   lines 320-326 with `let doc_id = Self::resolve_required_doc_id(exchange, "GET")?;`.
   Remove the old None-check block — `resolve_required_doc_id` handles it.

6. In `execute_delete` (line 345), replace the inline header extraction + None-check at
   lines 351-357 with `let doc_id = Self::resolve_required_doc_id(exchange, "DELETE")?;`.

7. In `execute_update` (line 376), replace the inline header extraction + None-check at
   lines 382-388 with `let doc_id = Self::resolve_required_doc_id(exchange, "UPDATE")?;`.

8. In `execute_exists` (line 458), replace the inline header extraction + None-check at
   lines 464-470 with `let doc_id = Self::resolve_required_doc_id(exchange, "EXISTS")?;`.

### Tests

- name: `validate_doc_id_rejects_path_separator`
  setup: None
  action: `let result = OpenSearchProducer::validate_doc_id("foo/bar");`
  assert: `assert!(result.is_err()); assert!(matches!(result.unwrap_err(), ProducerError::Permanent(_)))`
  command: `cargo test -p camel-opensearch --lib validate_doc_id_rejects_path_separator`
  expected: pass after implementation

- name: `validate_doc_id_rejects_query_separator`
  setup: None
  action: `let result = OpenSearchProducer::validate_doc_id("foo?bar");`
  assert: `assert!(result.is_err()); assert!(matches!(result.unwrap_err(), ProducerError::Permanent(_)))`
  command: `cargo test -p camel-opensearch --lib validate_doc_id_rejects_query_separator`
  expected: pass after implementation

- name: `validate_doc_id_rejects_fragment_separator`
  setup: None
  action: `let result = OpenSearchProducer::validate_doc_id("foo#bar");`
  assert: `assert!(result.is_err()); assert!(matches!(result.unwrap_err(), ProducerError::Permanent(_)))`
  command: `cargo test -p camel-opensearch --lib validate_doc_id_rejects_fragment_separator`
  expected: pass after implementation

- name: `validate_doc_id_rejects_percent`
  setup: None
  action: `let result = OpenSearchProducer::validate_doc_id("foo%2F");`
  assert: `assert!(result.is_err()); assert!(matches!(result.unwrap_err(), ProducerError::Permanent(_)))`
  command: `cargo test -p camel-opensearch --lib validate_doc_id_rejects_percent`
  expected: pass after implementation

- name: `validate_doc_id_rejects_null_byte`
  setup: None
  action: `let result = OpenSearchProducer::validate_doc_id("foo\0bar");`
  assert: `assert!(result.is_err()); assert!(matches!(result.unwrap_err(), ProducerError::Permanent(_)))`
  command: `cargo test -p camel-opensearch --lib validate_doc_id_rejects_null_byte`
  expected: pass after implementation

- name: `validate_doc_id_rejects_backslash`
  setup: None
  action: `let result = OpenSearchProducer::validate_doc_id("foo\\bar");`
  assert: `assert!(result.is_err()); assert!(matches!(result.unwrap_err(), ProducerError::Permanent(_)))`
  command: `cargo test -p camel-opensearch --lib validate_doc_id_rejects_backslash`
  expected: pass after implementation

- name: `validate_doc_id_rejects_dot_segment`
  setup: None
  action: Call `OpenSearchProducer::validate_doc_id(".")` and `OpenSearchProducer::validate_doc_id("..")`
  assert: `let r1 = OpenSearchProducer::validate_doc_id("."); let r2 = OpenSearchProducer::validate_doc_id(".."); assert!(r1.is_err()); assert!(r2.is_err()); assert!(matches!(r1.unwrap_err(), ProducerError::Permanent(_))); assert!(matches!(r2.unwrap_err(), ProducerError::Permanent(_)))`
  command: `cargo test -p camel-opensearch --lib validate_doc_id_rejects_dot_segment`
  expected: pass after implementation

- name: `validate_doc_id_rejects_control_char`
  setup: None
  action: Call `OpenSearchProducer::validate_doc_id("foo\u{0000}bar")` and `OpenSearchProducer::validate_doc_id("foo\u{007F}")`
  assert: `let r1 = OpenSearchProducer::validate_doc_id("foo\u{0000}bar"); let r2 = OpenSearchProducer::validate_doc_id("foo\u{007F}"); assert!(r1.is_err()); assert!(r2.is_err()); assert!(matches!(r1.unwrap_err(), ProducerError::Permanent(_))); assert!(matches!(r2.unwrap_err(), ProducerError::Permanent(_)))`
  command: `cargo test -p camel-opensearch --lib validate_doc_id_rejects_control_char`
  expected: pass after implementation

- name: `validate_doc_id_rejects_empty`
  setup: None
  action: `let result = OpenSearchProducer::validate_doc_id("");`
  assert: `assert!(result.is_err()); assert!(matches!(result.unwrap_err(), ProducerError::Permanent(_)))`
  command: `cargo test -p camel-opensearch --lib validate_doc_id_rejects_empty`
  expected: pass after implementation

- name: `validate_doc_id_rejects_oversized`
  setup: None
  action: `let result = OpenSearchProducer::validate_doc_id(&"a".repeat(513));`
  assert: `assert!(result.is_err()); assert!(matches!(result.unwrap_err(), ProducerError::Permanent(_)))`
  command: `cargo test -p camel-opensearch --lib validate_doc_id_rejects_oversized`
  expected: pass after implementation

- name: `validate_doc_id_accepts_valid_ids`
  setup: None
  action: Call `validate_doc_id("abc123")`, `validate_doc_id("user-42")`, `validate_doc_id("doc_001")`, `validate_doc_id("a.b.c")`, `validate_doc_id("type:id")`
  assert: `assert!(OpenSearchProducer::validate_doc_id("abc123").is_ok()); assert!(OpenSearchProducer::validate_doc_id("user-42").is_ok()); assert!(OpenSearchProducer::validate_doc_id("doc_001").is_ok()); assert!(OpenSearchProducer::validate_doc_id("a.b.c").is_ok()); assert!(OpenSearchProducer::validate_doc_id("type:id").is_ok())`
  command: `cargo test -p camel-opensearch --lib validate_doc_id_accepts_valid_ids`
  expected: pass after implementation

- name: `resolve_doc_id_wiring_rejects_poisoned_header`
  setup: Construct an `Exchange` with input header `CamelOpenSearch.Id` set to `Value::String("foo/bar".to_string())`. This is the exact extraction+validation path that `execute_index`, `execute_get`, `execute_delete`, `execute_update`, and `execute_exists` all call via `resolve_doc_id` or `resolve_required_doc_id`.
  action: `let result = OpenSearchProducer::resolve_doc_id(&exchange);`
  assert: `assert!(result.is_err()); assert!(matches!(result.unwrap_err(), ProducerError::Permanent(_)))`
  command: `cargo test -p camel-opensearch --lib resolve_doc_id_wiring_rejects_poisoned_header`
  expected: pass after implementation

- name: `resolve_doc_id_wiring_accepts_valid_header`
  setup: Construct an `Exchange` with input header `CamelOpenSearch.Id` set to `Value::String("abc123".to_string())`.
  action: `let result = OpenSearchProducer::resolve_doc_id(&exchange);`
  assert: `assert!(result.is_ok()); assert_eq!(result.unwrap(), Some("abc123"))`
  command: `cargo test -p camel-opensearch --lib resolve_doc_id_wiring_accepts_valid_header`
  expected: pass after implementation

- name: `resolve_doc_id_wiring_none_when_header_absent`
  setup: Construct an `Exchange` with no `CamelOpenSearch.Id` header.
  action: `let result = OpenSearchProducer::resolve_doc_id(&exchange);`
  assert: `assert!(result.is_ok()); assert_eq!(result.unwrap(), None)`
  command: `cargo test -p camel-opensearch --lib resolve_doc_id_wiring_none_when_header_absent`
  expected: pass after implementation

- name: `resolve_required_doc_id_wiring_missing_header_error`
  setup: Construct an `Exchange` with no `CamelOpenSearch.Id` header.
  action: `let result = OpenSearchProducer::resolve_required_doc_id(&exchange, "GET");`
  assert: `assert!(result.is_err()); assert!(matches!(result.unwrap_err(), ProducerError::Permanent(_)))`
  command: `cargo test -p camel-opensearch --lib resolve_required_doc_id_wiring_missing_header_error`
  expected: pass after implementation

### Acceptance

- `cargo test -p camel-opensearch --lib` passes with 0 failures
- `cargo clippy -p camel-opensearch -- -D warnings` exits 0
- `cargo fmt --check` passes
- All 5 operation call sites (`execute_index`, `execute_get`, `execute_delete`,
  `execute_update`, `execute_exists`) route through `resolve_doc_id` or
  `resolve_required_doc_id` instead of inline header extraction
- `cargo xtask lint-unwrap` passes (no new `unwrap()` introduced)

- [x] task-2

## Task 3: SQL verification tests

### Files

- `crates/components/camel-sql/src/producer.rs` (modified) — add verification tests only

### Steps

1. Verify existing tests `dynamic_query_denied_by_default` (producer.rs line 720) and
   `dynamic_query_allowed_with_opt_in` (producer.rs line 736) pass unchanged. These
   tests satisfy spec scenarios "SQL allow_dynamic_query defaults to false" and
   "SQL rejects header query text by default". The URI param name is `allowDynamicQuery`
   parsed via `params.get("allowDynamicQuery").map(|v| parse_bool_param("allowDynamicQuery", v)).transpose()?.unwrap_or(false)`
   at config.rs line 853.

2. Add one new verification test `sql_allow_dynamic_query_defaults_false_from_uri` that
   calls `SqlEndpointConfig::from_uri("sql:query?datasource=mydb&query=SELECT+1")` (no
   `allowDynamicQuery` param) and asserts `!config.allow_dynamic_query`.

### Tests

- name: `sql_allow_dynamic_query_defaults_false_from_uri`
  setup: `let config = SqlEndpointConfig::from_uri("sql:query?datasource=mydb&query=SELECT+1").unwrap();`
  action: Access `config.allow_dynamic_query`
  assert: `assert!(!config.allow_dynamic_query)`
  command: `cargo test -p camel-sql --lib sql_allow_dynamic_query_defaults_false_from_uri`
  expected: pass (existing behavior, new test)

### Acceptance

- `cargo test -p camel-sql --lib` passes with 0 failures
- `cargo clippy -p camel-sql -- -D warnings` exits 0
- `cargo fmt --check` passes

- [x] task-3
