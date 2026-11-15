# Tasks: txnreject

Single-phase change (no delivery phases; `design.md` has no `## Phases` section).

## camel-sql — reject at the component boundary

### Task 1.1: Add `SqlEndpointConfig::validate_transaction_mode` and reject `Managed` in `create_endpoint`

**Files:**
- `crates/components/camel-sql/src/config.rs` (modified)
- `crates/components/camel-sql/src/lib.rs` (modified)

**Steps:**
1. In `config.rs`, inside `impl SqlEndpointConfig`, add:
   `pub fn validate_transaction_mode(&self, uri: &str) -> Result<(), CamelError>`.
   On `self.transaction_mode == TransactionMode::Managed` it returns
   `Err(CamelError::Config(format!(` with this message text (exact wording,
   `{}` filled with `camel_api::redact::redact_url(uri)` — never the raw URI:
   SQL URIs carry credentials in `db_url`):
   `transactionMode=Managed is not implemented (bd rc-qv2iz); rejected endpoint '{}'. Use transactionMode=Auto with a single multi-statement sql: step: on statement failure the call is rolled back and the connection is reused; the connection is discarded only if the rollback itself fails or cleanup is cancelled.`
   On `TransactionMode::Auto` it returns `Ok(())`. The `uri` parameter is used
   only to name the endpoint in the message.
2. In `lib.rs`, `SqlComponent::create_endpoint`, insert
   `config.validate_transaction_mode(uri)?;` immediately after
   `let mut config = SqlEndpointConfig::from_uri(uri)?;` and BEFORE the
   `datasource_name` catalog check (Managed must be rejected before catalog
   lookup and health-check registration).
3. Do NOT change `TransactionMode::from_str`: it must keep accepting `"Managed"`.
4. Do NOT change `SqlEndpoint::new*` constructors.

**Tests:** (name, setup, action, assert; command runs each)
- `create_endpoint_rejects_managed_transaction_mode`: setup `SqlComponent::new()` + `NoOpComponentContext` (existing test style in `lib.rs` `mod tests`) → action `let Err(e) = c.create_endpoint("sql:select 1?db_url=postgres://user:s3cr3tpass@localhost/test&transactionMode=Managed", &ctx) else { panic!("expected rejection") };` (bind the error without `unwrap_err()`; the success type is not `Debug`) → assert `e.to_string()` contains `"Managed"`, `"rc-qv2iz"`, and `"sql:select 1"`, and does NOT contain `"s3cr3tpass"` (secret-redacted URI). Command: `cargo test -p camel-component-sql --lib create_endpoint_rejects_managed_transaction_mode`. Expected before implementation: FAIL (today the call succeeds).
- `create_endpoint_accepts_auto_transaction_mode`: same setup → action `c.create_endpoint("sql:select 1?db_url=postgres://localhost/test&transactionMode=Auto", &ctx)` → assert `result.is_ok()`. Command: `cargo test -p camel-component-sql --lib create_endpoint_accepts_auto_transaction_mode`. Expected before implementation: PASS (positive control; must stay passing).
- `managed_reject_is_config_error_not_parse_error`: same setup → action `let Err(e) = c.create_endpoint("sql:select 1?db_url=postgres://localhost/test&transactionMode=Managed", &ctx) else { panic!("expected rejection") };` → assert `matches!(e, CamelError::Config(_))` (NOT `CamelError::InvalidUri`) and `e.to_string()` contains `"transactionMode=Auto"` and `"multi-statement"`. Command: `cargo test -p camel-component-sql --lib managed_reject_is_config_error_not_parse_error`. Expected before implementation: FAIL.
- Existing `transaction_mode_managed_from_uri` in `config.rs` `mod tests` (parse guard: `from_uri` with `transactionMode=Managed` returns `Ok` with `TransactionMode::Managed`) must stay green. Command: `cargo test -p camel-component-sql --lib transaction_mode_managed_from_uri`. Expected: PASS before and after.

**Acceptance:**
- `cargo test -p camel-component-sql --lib` exits 0 (all tests green).
- `cargo clippy -p camel-component-sql --all-targets -- -D warnings` exits 0.
- `cargo fmt --check --all` exits 0.
- No `unwrap()` introduced in non-test code (`cargo xtask lint-unwrap` exits 0).

- [x] 1.1

### Task 1.2: Comment the unreachable warn paths and correct the `TransactionMode` doc comment

**Files:**
- `crates/components/camel-sql/src/config.rs` (modified)
- `crates/components/camel-sql/src/endpoint.rs` (modified)
- `crates/components/camel-sql/src/producer.rs` (modified)
- `crates/components/camel-sql/src/consumer.rs` (modified)

**Steps:**
1. `config.rs` `TransactionMode` enum doc comment: replace the `Managed` bullet
   ("Explicit transaction boundaries (future; currently logs a warning and falls
   back to Auto)") with: "Explicit transaction boundaries (not implemented).
   `SqlComponent::create_endpoint` rejects this value with a `CamelError::Config`
   naming bd `rc-qv2iz`; direct `SqlEndpoint::new*` construction bypasses that
   validation, so the producer/consumer safety-net warn still applies there."
   Keep the SQL-002 task comment above the enum and the `#[default] Auto` bullet unchanged.
2. `endpoint.rs`: above the `SqlEndpoint::new*` constructors add one short
   comment: URI-surface construction validates `transactionMode` in
   `SqlEndpointConfig::validate_transaction_mode` (called from
   `SqlComponent::create_endpoint`); these constructors take a config by value
   and cannot reject, so programmatic callers may still pass `Managed`.
3. `producer.rs` (the `// SQL-002: warn if Managed transaction mode requested`
   comment above the `if config.transaction_mode == crate::config::TransactionMode::Managed`
   branch at ~line 251): replace that comment with: "SQL-002: unreachable through the URI
   surface — `create_endpoint` rejects `transactionMode=Managed` first
   (`validate_transaction_mode`). Kept as a safety net for programmatic
   construction via `SqlEndpoint::new*`, which bypasses validation." Keep the
   `if` branch and its `warn!` call byte-identical.
4. `consumer.rs` (same pattern at ~line 498): same replacement comment, same
   rule — keep the `if` branch and `warn!` unchanged.
5. No behaviour change in this task: only doc comments change.

**Tests:**
- `cargo test -p camel-component-sql --lib` runs the existing suite including
  `producer`/`consumer` unit tests → assert all green (behaviour unchanged).
  Command: `cargo test -p camel-component-sql --lib`. Expected: PASS before and after.
- `cargo clippy -p camel-component-sql --all-targets -- -D warnings` → exit 0 (comment
  edits introduce no lint delta).

**Acceptance:**
- `git diff` for this task touches only comments and doc comments (no `warn!`,
  `if`, or executable statement changed).
- `cargo test -p camel-component-sql --lib` exits 0.
- `cargo clippy -p camel-component-sql --all-targets -- -D warnings` exits 0.

- [x] 1.2

## docs — database.md Managed paragraph

### Task 1.3: Document the reject in `docs/src/components/database.md`

**Files:**
- `docs/src/components/database.md` (modified)

**Steps:**
1. In the paragraph on manual multi-statement transactions (line ~44), replace
   the final sentence "`transactionMode=Managed` is still pending (bd rc-qv2iz)
   and is not available." with: "The component REJECTS
   `transactionMode=Managed` at load with an error naming bd `rc-qv2iz`; use
   `transactionMode=Auto` with a single multi-statement call (one `sql:` step),
   as described above, until `rc-qv2iz` lands." Keep the rest of the paragraph
   (rollback/discard semantics) byte-identical.
2. Check the rest of `docs/src/components/database.md` for any other claim that
   Managed is "pending" or "available"; if one exists, update it the same way
   (report it in the task result if found).

**Tests:**
- `cd docs && mdbook test` compiles and runs every doc test in the book → exit
  0. Command: `cd docs && mdbook test`. Expected: PASS before and after
  (prose-only change).
- `docs_managed_paragraph_states_reject`: assert the Managed sentence in
  `docs/src/components/database.md` changed. Command:
  `grep -c "The component REJECTS" docs/src/components/database.md` → expect 1
  (FAIL before the edit: 0), `grep -c "still pending" docs/src/components/database.md`
  → expect 0 (FAIL before the edit: 1), and
  `grep -c "transactionMode=Auto" docs/src/components/database.md` → expect ≥ 1
  (FAIL before the edit: 0).

**Acceptance:**
- `cd docs && mdbook test` exits 0.
- The paragraph states the reject, names `rc-qv2iz`, and keeps the
  single-statement workaround.
- No other file changed in this task.

- [x] 1.3
