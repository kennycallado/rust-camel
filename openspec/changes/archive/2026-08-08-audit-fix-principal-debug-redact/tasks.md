# Tasks: audit-fix-principal-debug-redact

## camel-api

### Task 1.1: Manual Debug impl redacting untrusted claims on Principal

**Files:**
- `crates/camel-api/src/security_policy.rs` (modified)

**Steps:**
1. In `security_policy.rs`, change the derive on `pub struct Principal` (line 12)
   from `#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]` to
   `#[derive(Clone, PartialEq, Serialize, Deserialize)]` — remove `Debug`.
2. Add a manual `impl std::fmt::Debug for Principal` immediately after the
   `impl Principal { ... }` block (after the closing brace of the `has_scope`
   method, around line 34). The impl uses `f.debug_struct("Principal")` and
   emits `.field("subject", &self.subject)`, `.field("issuer", &self.issuer)`,
   `.field("audience", &self.audience)`, `.field("scopes", &self.scopes)`,
   `.field("roles", &self.roles)`, `.field("claims", &"[REDACTED]")`, then
   `.finish()`. This mirrors the established `ServerTlsConfig` pattern in
   `crates/components/camel-http/src/config.rs:90-97`.
3. In the existing `#[cfg(test)] mod tests` block (starts at line 147), add the
   three regression tests specified in the Tests section below.
4. Run `cargo fmt -p camel-api`, then `cargo check -p camel-api`, then
   `cargo clippy -p camel-api -- -D warnings`, then
   `cargo test -p camel-api --lib`.

**Tests:** (executable spec)

- `principal_debug_redacts_claims_compact`:
  - setup: build a `Principal` with `subject = "subj-1"`, `issuer = "iss"`,
    `audience = vec!["a1".into()]`, `scopes = vec!["s1".into()]`,
    `roles = vec!["r1".into()]`, and `claims = serde_json::json!({"piid":
    "SENTINEL_CLAIM_VALUE_9kq2"})` (the sentinel appears in NO retained
    descriptor).
  - action: `let s = format!("{principal:?}");` (compact Debug)
  - assert: `s.contains("claims: \"[REDACTED]\"")` is true,
    `s.contains("SENTINEL_CLAIM_VALUE_9kq2")` is false, and ALL five retained
    descriptor values are present: `s.contains("subj-1")`, `s.contains("iss")`,
    `s.contains("a1")`, `s.contains("s1")`, `s.contains("r1")` are each true.
- `principal_debug_redacts_claims_pretty`:
  - setup: same `Principal` as above.
  - action: `let s = format!("{principal:#?}");` (pretty Debug)
  - assert: `s.contains("[REDACTED]")` is true and
    `s.contains("SENTINEL_CLAIM_VALUE_9kq2")` is false.
- `principal_serialize_preserves_claims`:
  - setup: same `Principal` as above (sentinel-bearing claims).
  - action: `let s = serde_json::to_string(&principal).unwrap();`
  - assert: `s.contains("SENTINEL_CLAIM_VALUE_9kq2")` is true — claims survive
    serde serialization (the redaction is Debug-only, not Serialize).
- command: `cargo test -p camel-api --lib principal_`
- expected: the two Debug tests fail before step 1-2 (the derived Debug prints
  the raw claim sentinel) and pass after the manual impl lands. The serialize
  test passes both before and after (it guards against a future regression that
  adds `#[serde(skip)]` to claims).

**Acceptance:**
- `cargo clippy -p camel-api -- -D warnings` exits 0.
- `cargo check -p camel-api` exits 0.
- `cargo test -p camel-api --lib` passes, including the three new
  `principal_*` tests (two Debug redaction + one serialize-preservation).
- `cargo fmt --check` exits 0.
- `cargo xtask lint-secrets` exits 0 (the redaction is consistent with the
  ADR-0051 lint posture; Principal.claims is untrusted data, not a credential,
  but the manual Debug must not trip the lint).
- Spec coverage: exercises all three scenarios in
  `specs/security/spec.md` (compact redaction → `principal_debug_redacts_claims_compact`,
  pretty redaction → `principal_debug_redacts_claims_pretty`,
  serialization unaffected → `principal_serialize_preserves_claims`).

- [x] 1.1
