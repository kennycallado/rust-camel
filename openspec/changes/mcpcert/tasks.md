# Tasks: mcpcert — reject blank MCP TLS paths in ROUTE_SCHEMA

Single-phase change (design.md ## Phases). Two tasks. Task 1.1 owns the
schema constraint + unit tests; Task 1.2 owns the corpus fixture +
baseline. Ordering: implement 1.1 BEFORE 1.2 — 1.2's corpus gate is red
until 1.1 lands (without 1.1 the fixture emits nothing → MISSING-
REGRESSION arm fails; with 1.1 but no baseline entry the FALSE-POSITIVE
arm fails).

## Task 1.1 — schema constraint + R-SCHEMA unit tests

- **Files:**
  - `crates/camel-dsl/src/mcp.rs` (modified) — add
    `#[cfg_attr(feature = "schema", schemars(regex(pattern = r"\S")))]`
    above BOTH `cert_path` and `key_path` in `struct RouteDslMcpTlsConfig`
    (keep the existing `#[serde(deserialize_with = "deserialize_cert_path")]`
    / `#[serde(deserialize_with = "deserialize_key_path")]` lines and doc
    comments; attribute order: schemars cfg_attr first, then serde).
  - `schemas/dsl/route-schema.json` (modified, generated) — via
    `cargo xtask schema`.
  - `crates/camel-lint/schema/route-schema.json` (modified, generated) —
    byte-synced by the same command.
  - `crates/camel-lint/src/rules/rschema.rs` (modified) — AnyOf
    de-collapse arm in the `analyze` error loop (see step 3b).
  - `crates/camel-lint/src/rules/rschema/tests.rs` (modified) — 6 new
    `#[test]` fns, placed after `rschema_mcp_form_missing_bind_reports_parent`.

- **Steps:**
  1. In `crates/camel-dsl/src/mcp.rs`, add the cfg_attr schemars regex
     line to `cert_path` and `key_path` of `RouteDslMcpTlsConfig`:
     `#[cfg_attr(feature = "schema", schemars(regex(pattern = r"\S")))]`
     (raw string, exactly this nested `regex(pattern = ...)` form — the
     bare `regex = "..."` form does not compile on schemars 1.2.2).
  2. From the worktree root run `cargo xtask schema` (mutating mode) —
     regenerates `schemas/dsl/route-schema.json` and syncs
     `crates/camel-lint/schema/route-schema.json`. Verify with
     `git diff --stat`: exactly those two schema files changed, and each
     shows `"pattern": "\\S"` added to the two
     `RouteDslMcpTlsConfig.properties` fields (inspect with
     `python3 -m json.tool` or `git diff`).
  3. In `crates/camel-lint/src/rules/rschema.rs`, extend the error-loop
     `match err.kind()` with an arm BEFORE the catch-all `_` arm:
     `ValidationErrorKind::AnyOf { context }` → flatten `context`
     (its type is `Vec<Vec<ValidationError<'static>>>`, one Vec per
     anyOf branch, each holding nested owned errors with their own
     `instance_path()`/`kind()`), filter nested errors whose kind is
     `ValidationErrorKind::Pattern { .. }` AND whose `instance_path()`
     is STRICTLY deeper (more segments) than the anyOf error's own
     `instance_path`. Dedup the survivors by
     (instance_path, pattern-string). If any survive: for each, resolve
     the span via the existing
     `instance_path_to_noyalib(nested.instance_path().as_str(), envelope_depth)`
     + `value_span_for` chain and push `diagnostic_for(span, nested.to_string())`
     (its Display is `{instance} does not match "{pattern}"`), REPLACING
     the collapsed anyOf diagnostic for this node. If none survive: emit
     exactly today's behavior (the existing `_`-arm diagnostic —
     collapsed anyOf error at the anyOf node). Document the arm with a
     comment: pre-existing collapse limitation, de-collapsed for Pattern
     only because the schema's only pattern keywords are the two MCP TLS
     path fields (rc-n3t73).
  4. Add the 6 tests below to
     `crates/camel-lint/src/rules/rschema/tests.rs`, following the file's
     existing `analyze(source)` + `rschema_only(&diags)` +
     `slice(source, &d.span)` helper pattern (see
     `rschema_mcp_form_defect_anchors_value` for the template).
  5. Run `cargo fmt` on touched crates; run
     `cargo clippy -p camel-lint -p camel-dsl -- -D warnings`; run the
     new tests.

- **Tests (all in crates/camel-lint/src/rules/rschema/tests.rs):**

  Assert canon (applies to tests 1-4 and 6 — read before writing):
  - jsonschema 0.52.1's `pattern` error Display is
    `{instance} does not match "{pattern}"` — the message contains
    `does not match` and `\S`, but NEVER the field name (`instance_path`
    is used for span resolution, not the message). So field
    identification is SPAN-based: each test has exactly ONE blank
    field, so assert `rschema.len() == 1` and the single error's span
    slice is the blank value — that pins which field erred.
  - IMPORTANT (empirical, verified against jsonschema 0.52.1): without
    step 3 (the AnyOf de-collapse arm), the pattern violation inside
    `tls` COLLAPSES to one anyOf error anchored at the whole tls
    mapping (`"... is not valid under any of the schemas ..."`). The
    tests below assert the POST-step-3 shape: leaf-anchored,
    `does not match` message. Red phase = tests fail with the collapsed
    shape; green phase = leaf shape after step 3.
  - `value_span_for` returns the RAW YAML scalar token, quotes
    INCLUDED for quoted scalars (document.rs `unquoted_span` doc).
    So `cert_path: ""` anchors with `slice == "\"\""`, and
    `cert_path: "   "` with `slice == "\"   \""` (quote + 3 spaces +
    quote). Assert blank-ness robustly:
    `slice(source, &d.span).trim_matches('"').trim().is_empty()` plus
    (optionally) the exact raw slice.
  - Message assert: `d.message.contains("does not match")` (pattern
    violation shape).

  1. name: `rschema_mcp_tls_blank_cert_path_empty_rejected`
     - setup: mcp-block source (name: crm, bind: 127.0.0.1:9100) whose
       `tls.cert_path` is `""` and `tls.key_path` is
       `/etc/certs/crm-key.pem`.
     - action: `analyze(source)`, filter `rschema_only`.
     - assert: `rschema.len() == 1`; the single Error's span slice
       `trim_matches('"').trim().is_empty()` (the blank cert value —
       with key_path valid, the only possible error site is
       cert_path); message contains `does not match`.
     - command: `cargo test -p camel-lint rschema_mcp_tls_blank_cert_path_empty_rejected`
     - expected: FAIL before steps 1-3 (collapsed anyOf shape), PASS after.

  2. name: `rschema_mcp_tls_blank_cert_path_whitespace_rejected`
     - same as test 1 but `tls.cert_path` is `"   "` (three spaces);
       same asserts (`len == 1`, blank slice, pattern message).
     - command: `cargo test -p camel-lint rschema_mcp_tls_blank_cert_path_whitespace_rejected`

  3. name: `rschema_mcp_tls_blank_key_path_empty_rejected`
     - mirror of test 1 with `tls.key_path: ""` and a valid
       `cert_path: /etc/certs/crm.pem`; same asserts (single error,
       blank slice, pattern message) — with cert_path valid the only
       error site is key_path.
     - command: `cargo test -p camel-lint rschema_mcp_tls_blank_key_path_empty_rejected`

  4. name: `rschema_mcp_tls_blank_key_path_whitespace_rejected`
     - mirror of test 2 with `tls.key_path: "   "`; same asserts.
     - command: `cargo test -p camel-lint rschema_mcp_tls_blank_key_path_whitespace_rejected`

  5. name: `rschema_mcp_tls_padded_valid_path_stays_silent`
     - setup: full mcp block with `tls.cert_path: " /etc/certs/crm.pem "`
       (leading+trailing spaces around a real path) and valid key_path.
     - action: `analyze(source)`, filter `rschema_only`.
     - assert: empty (runtime trims and accepts; `\S` matches).
     - command: `cargo test -p camel-lint rschema_mcp_tls_padded_valid_path_stays_silent`

  6. name: `rschema_mcp_tls_blank_cert_path_empty_env_default_rejected`
     - setup: mcp block with `tls.cert_path: "${env:CERT:-}"` (whole-scalar
       token, EMPTY default) and valid `key_path: /etc/certs/crm-key.pem`.
     - action: `analyze(source)`, filter `rschema_only`.
     - assert: at least one Error whose span slice contains
       `${env:CERT:-}` (the rc-93wct interpolated copy substitutes the
       empty default to `""`, which fails the pattern — the diagnostic
       anchors on the AUTHORED token; boot substitutes then rejects,
       parity) and whose message contains `does not match`; no error
       anchored anywhere else (key_path is valid).
     - command: `cargo test -p camel-lint rschema_mcp_tls_blank_cert_path_empty_env_default_rejected`
     - expected: FAIL before steps 1-3 (collapsed anyOf shape), PASS after.

  Note on quoting: for blank values use YAML quoted forms
  (`cert_path: ""`, `cert_path: "   "`) — the span anchors on the raw
  token INCLUDING the quotes (see assert canon above).

- **Acceptance:**
  - `git diff` shows the two schema files gained
    `"pattern": "\\S"` on `RouteDslMcpTlsConfig` cert_path/key_path and
    nothing else changed in them.
  - `cargo xtask schema --check` exits 0 (no drift between regenerated
    and committed artifacts, incl. the camel-lint embedded copy).
  - `cargo test -p camel-lint rschema_mcp_tls` passes (6 tests).
  - `cargo test -p camel-lint --lib` passes (existing suite unbroken —
    in particular the existing anyOf-collapse diagnostics for NON-pattern
    defects stay byte-identical: no existing test changes).
  - `cargo fmt --check` clean on touched crates;
    `cargo clippy -p camel-lint -p camel-dsl -- -D warnings` exits 0.
  - Spec scenarios covered: "Empty cert_path is rejected and anchored on
    the value", "Whitespace-only key_path is rejected" (tests 1-4),
    "Trimmed-valid path stays silent" (test 5), "Blank path via empty
    env default is rejected" (test 6).

- [x] 1.1

## Task 1.2 — corpus negative fixture + baseline entry

- **Files:**
  - `crates/camel-cli/tests/fixtures/lint-corpus/mcp-tls-blank-paths.yaml`
    (new) — negative sibling of `mixed-routes-rest-mcp.yaml` (39fd3bc5).
  - `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron`
    (modified) — one new entry with justification comment.

- **Steps:**
  1. Create the fixture: a minimal mcp-block document (modeled on the
     mcp section of `mixed-routes-rest-mcp.yaml`) with
     `tls.cert_path: ""` and `tls.key_path: "   "` (both blank classes
     in one file). Header comment (English) explaining: negative
     witness for rc-n3t73 — blank TLS paths must FAIL R-SCHEMA; sibling
     of mixed-routes-rest-mcp.yaml (rc-ysvjl, 39fd3bc5); carries an
     agreed real defect, hence baselined.
  2. Add the baseline entry to
     `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron`:
     `("crates/camel-cli/tests/fixtures/lint-corpus/mcp-tls-blank-paths.yaml", [("R-SCHEMA", "error")])`
     with a `//` justification comment: blank cert_path + whitespace
     key_path are real defects (runtime non_empty_path rejects both);
     pinning the lint-side rejection (rc-n3t73).
  3. Run the corpus gate and confirm green.

- **Tests:**
  - name: `corpus_zero_false_positives` (EXISTING test — this task makes
    it cover the new fixture)
    - setup: new fixture on disk emitting R-SCHEMA Error(s) after Task
      1.1; baseline entry present.
    - action: `cargo test -p camel-cli --test lint_corpus corpus_zero_false_positives`
    - assert: gate passes — the fixture's emitted `("R-SCHEMA", "error")`
      matches the baseline entry exactly (two blank fields collapse to
      one (code,severity) pair by set semantics).
    - expected: before Task 1.1 the same fixture would emit NOTHING and
      the MISSING-REGRESSION arm would fail; after Task 1.1 without the
      baseline entry the FALSE-POSITIVE arm fails. Both arms green only
      when Task 1.1 + this task land together.

- **Acceptance:**
  - `cargo test -p camel-cli --test lint_corpus` passes (whole corpus
    suite, including the probe/self-check tests).
  - Baseline entry carries a justification comment.
  - Fixture file is valid YAML (mcp envelope shape, depth-0 form).
  - Spec scenario covered: "Corpus negative fixture is baselined as
    failing".

- [x] 1.2
