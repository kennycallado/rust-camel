# Tasks: rsiblings

## camel-lint

### Task 1.1: Append non-pattern sibling leaf diagnostics in the anyOf pattern de-collapse pass

**Files:**
- `crates/camel-lint/src/rules/rschema.rs` (modified)
- `crates/camel-lint/src/rules/rschema/tests.rs` (modified)

**Steps:**
1. Write the four new tests listed under Tests FIRST, in
   `crates/camel-lint/src/rules/rschema/tests.rs`, next to the
   existing `rschema_mcp_tls_*` family (they mirror that family's
   `analyze` / `rschema_only` / `slice` helper pattern). Run
   `cargo test -p camel-lint --lib rschema_mcp_tls` (family-wide
   filter) and confirm the pre-impl state: the three co-occurrence
   tests FAIL (sibling diagnostics missing / count wrong); the
   pure-non-pattern collapsed-unchanged pin test PASSES (it pins
   today's behavior — regression guard, this is expected); the four
   existing blank-path tests PASS (regression guards, expected).
2. In `rschema.rs`, extract the sibling collection into a small named
   helper next to `collect_permission_oneof_paths` (mirroring that
   walker's shape), keeping emission inline in the arm to preserve
   the pattern-first ordering coupling:
   - `fn collect_sibling_errors<'a>(err: &'a ValidationError<'_>, own_depth: usize, out: &mut Vec<&'a ValidationError<'a>>)`
     walking the same `context` branches of the collapsed AnyOf
     error: skip nested errors whose kind is `Pattern` (already
     handled by the pattern walk); skip a nested error when BOTH its
     kind is `ValidationErrorKind::Type` AND its
     `instance_path().as_str()` equals the collapsed error's own
     `instance_path` — that is the `{"type": "null"}` sibling branch
     failing because the instance is an object of the intended shape
     (branch noise, not a defect); skip nested errors whose segment
     depth is strictly LESS than the collapsed node's own depth
     (defensive; jsonschema reports nested branch errors with
     absolute deeper-or-equal paths); dedup by
     `(instance_path, diagnostic_message(nested))` preserving
     first-occurrence order (branches retry the same subschema
     shapes).
3. In the arm's pass 2, after the `pattern_errors` collection loop:
   call the helper (only on the path where `pattern_errors` is
   non-empty — the collapsed emission branch stays untouched), then
   emit the pattern leaves exactly as today, THEN for each `nested`
   in `sibling_errors`:
   - If `nested.kind()` is `ValidationErrorKind::AdditionalProperties
     { unexpected }`: mirror the top-level arm — resolve each
     unexpected key's span via `crate::document::key_span_for` with
     `key_path` = parent `instance_path_to_noyalib(nested.instance_path(),
     envelope_depth)` joined with the key (same `format!` shape as
     the top-level arm), and push one
     `diagnostic_for(span, diagnostic_message(nested))` per key.
   - Otherwise anchor via `value_span_for` on
     `instance_path_to_noyalib(nested.instance_path(), envelope_depth)`
     and push `diagnostic_for(span, diagnostic_message(nested))`
     (same shape as the `_` arm).
   When `pattern_errors` is empty, do NOT collect or emit siblings —
   the collapsed diagnostic stays byte-identical.
4. Rewrite the "KNOWN LIMITATION (pattern pass)" comment block in the
   arm: the limitation is LIFTED — co-located non-pattern defects now
   surface as sibling leaf diagnostics; null-branch whole-node type
   mismatches are excluded as branch noise. Extend the module-header
   anyOf bullet (rschema.rs lines ~33-36, which today describes only
   the permission de-collapse) to also state the sibling surfacing.
   The permission-pass (pass 1) KNOWN LIMITATION comment stays
   unchanged.
5. Run the full camel-lint lib suite, the workspace-lib suite, fmt,
   and clippy (commands below). All green.

**Tests:** (executable spec — name, arrange, act, assert)
- `rschema_mcp_tls_blank_cert_path_unknown_key_sibling_both_reported`:
  source has `tls: {cert_path: "", key_path: /etc/certs/crm-key.pem,
  rogue_key: true}` → `analyze` → `rschema_only` → assert
  `rschema.len() == 2`; `rschema[0]` slice `== "\"\""` and message
  `.contains("does not match")` (pattern leaf); `rschema[1]` message
  `.contains("Additional properties are not allowed")` and slice
  `.contains("rogue_key")` (sibling anchored on the unknown key);
  NO diagnostic message `.contains("is not valid under any of the
  schemas")` (no collapsed container diagnostic, no null-branch
  noise).
- `rschema_mcp_tls_blank_cert_path_nonstring_key_path_sibling_both_reported`:
  source has `tls: {cert_path: "", key_path: []}` → `analyze` →
  `rschema_only` → assert `rschema.len() == 2`; `rschema[0]` slice
  `== "\"\""` and message `.contains("does not match")`;
  `rschema[1]` slice `== "[]"` and message `.contains("is not of
  type")` and `.contains("string")` (deeper Type sibling on the
  offending value); NO message `.contains("is not valid under any of
  the schemas")`.
- `rschema_mcp_tls_blank_paths_unknown_key_three_defects_reported`:
  source has `tls: {cert_path: "", key_path: "   ", rogue_key: 1}`
  (both patterns + one unknown key) → assert `rschema.len() == 3`:
  two `does not match` leaves (slices `"\"\""` and `"\"   \""`) plus
  one `Additional properties are not allowed` sibling; NO collapsed
  container message.
- `rschema_mcp_tls_unknown_key_alone_collapsed_unchanged`:
  source has `tls: {cert_path: /a.pem, key_path: /b.pem, rogue:
  true}` (non-pattern only) → assert `rschema.len() == 1` and
  `rschema[0].message == "{\"cert_path\":\"/a.pem\",\"key_path\":\"/b.pem\",\"rogue\":true} is not valid under any of the schemas listed in the 'anyOf' keyword"`
  (byte-exact canonical_json pin) and slice
  `.contains("rogue: true")` (anchored on the whole tls mapping).
- Pure-pattern byte-identical regression (no new test needed — the
  existing `rschema_mcp_tls_blank_cert_path_empty_rejected`,
  `rschema_mcp_tls_blank_cert_path_whitespace_rejected`,
  `rschema_mcp_tls_blank_key_path_empty_rejected`,
  `rschema_mcp_tls_blank_key_path_whitespace_rejected` must stay
  green UNCHANGED, still asserting exactly one diagnostic each): run
  `cargo test -p camel-lint --lib rschema_mcp_tls` and confirm all
  family tests pass.

**Commands:**
- `cargo test -p camel-lint --lib rschema_mcp_tls` — pre-impl state
  per step 1 (three co-occurrence tests FAIL; pin + regression
  guards PASS); all family tests PASS after steps 2-4.
- `cargo test -p camel-lint --lib` — all green after (includes
  `rschema_exception_disposition_oneof_unchanged`,
  `rschema_rest_binding_oneof_unchanged` byte-exact pins).
- `cargo test --workspace --lib` — all green after (byte-exact class
  depends on build graph: workspace feature unification of
  `preserve_order` is the exact drift `canonical_json` guards).
- `cargo fmt --check` and `cargo clippy -p camel-lint --all-targets
  -- -D warnings` — clean after.

**Acceptance:**
- `cargo test -p camel-lint --lib` exits 0.
- `cargo test --workspace --lib` exits 0.
- The three co-occurrence tests and the collapsed-unchanged pin test
  exist and pass; the four existing blank-path tests are untouched
  and pass.
- `cargo clippy -p camel-lint --all-targets -- -D warnings` exits 0.
- `cargo fmt --check` exits 0.

- [x] 1.1

## camel-cli (corpus)

### Task 1.2: Corpus co-occurrence fixture with justified baseline entry

**Files:**
- `crates/camel-cli/tests/fixtures/lint-corpus/mcp-tls-sibling-defects.yaml` (new)
- `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` (modified)

**Steps:**
1. Create `mcp-tls-sibling-defects.yaml` modeled on the existing
   `mcp-tls-blank-paths.yaml` (same document shape, header comment
   citing rc-lys6a): one mcp server with
   `tls: {cert_path: "", key_path: /etc/certs/crm-key.pem,
   rogue_key: true}` — the pattern + additionalProperties
   co-occurrence witness. Header comment states the justification:
   both defects are real boot failures (`non_empty_path`,
   `deny_unknown_fields`), so the R-SCHEMA Errors are agreed real
   defects, hence baselined.
2. Add a baseline entry to `lint-corpus-baseline.ron` directly after
   the `mcp-tls-blank-paths.yaml` entry, same shape:
   `("crates/camel-cli/tests/fixtures/lint-corpus/mcp-tls-sibling-defects.yaml",
   [("R-SCHEMA", "error")]),` with a leading `//` comment line
   explaining the co-occurrence witness (rc-lys6a: sibling
   additionalProperties defect must surface alongside the pattern
   leaf).
3. Run the corpus test; confirm green.

**Tests:**
- `corpus_zero_false_positives` (existing gate test): new fixture
  present + baselined → `cargo test -p camel-cli --test lint_corpus`
  passes (exit 0). Before the baseline entry exists the gate FAILS
  (unjustified diagnostics) — write fixture first, watch it fail,
  then add the entry and watch it pass.

**Acceptance:**
- `cargo test -p camel-cli --test lint_corpus` exits 0.
- Baseline entry carries a justification comment; fixture carries a
  header comment citing rc-lys6a.
- `cargo fmt --check` exits 0 (fixture/baseline are not Rust; the
  check guards against accidental Rust edits only).

- [x] 1.2
