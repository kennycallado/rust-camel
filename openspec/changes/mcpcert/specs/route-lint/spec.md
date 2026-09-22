# route-lint delta — mcpcert

## ADDED Requirements

### Requirement: R-SCHEMA rejects blank MCP TLS path values with runtime parity

The generated ROUTE_SCHEMA MUST reject empty and whitespace-only values
for `mcp[].server.tls.cert_path` and `mcp[].server.tls.key_path`,
mirroring the runtime `non_empty_path` deserializer (trim + reject
empty). The rejection MUST surface as an R-SCHEMA Error anchored on the
offending value (the anchor pins the offending field), carrying a
pattern-violation message. Values that are non-empty after trimming
MUST NOT be flagged (runtime trims and accepts them).

#### Scenario: Empty cert_path is rejected and anchored on the value

- **Given** an `mcp:` block whose `server.tls.cert_path` is the empty
  string `""` and `key_path` is a valid path
- **When** R-SCHEMA analyzes the document
- **Then** exactly one R-SCHEMA Error is emitted, anchored on the blank
  `cert_path` value with a pattern-violation message, and no Error is
  anchored on `key_path`

#### Scenario: Whitespace-only key_path is rejected

- **Given** an `mcp:` block whose `server.tls.key_path` is
  whitespace-only (e.g. `"   "`) and `cert_path` is a valid path
- **When** R-SCHEMA analyzes the document
- **Then** exactly one R-SCHEMA Error is emitted, anchored on the blank
  `key_path` value with a pattern-violation message, and no Error is
  anchored on `cert_path`

#### Scenario: Blank path via empty env default is rejected

- **Given** an `mcp:` block whose `server.tls.cert_path` is the
  whole-scalar token `${env:CERT:-}` (empty default)
- **When** R-SCHEMA analyzes the document against the interpolated copy
- **Then** an R-SCHEMA Error is emitted, anchored on the authored token,
  for the blank substituted value (boot substitutes then rejects it —
  parity)

#### Scenario: Trimmed-valid path stays silent

- **Given** an `mcp:` block whose `server.tls.cert_path` is
  `" /etc/certs/crm.pem "` (leading/trailing spaces around a real path)
- **When** R-SCHEMA analyzes the document
- **Then** no R-SCHEMA diagnostic is emitted for that value (runtime
  trims and accepts)

#### Scenario: Corpus negative fixture is baselined as failing

- **Given** the corpus fixture
  `crates/camel-cli/tests/fixtures/lint-corpus/mcp-tls-blank-paths.yaml`
  carrying a blank `cert_path` and a whitespace-only `key_path`
- **When** the `corpus_zero_false_positives` gate runs
- **Then** the fixture emits `("R-SCHEMA", "error")` exactly as recorded
  in the baseline with a justification, and the gate passes

## Self-grill record

**Questions generated:**
1. [glossary] Do the scenarios use the canonical R-SCHEMA
   diagnostic/severity vocabulary (Error, instance-path anchoring) as the
   route-lint spec defines it?
2. [sharpen] Scenario "Blank path via empty env default" asserts the
   `${env:CERT:-}` token is rejected. Is that the interpolated-copy path,
   and does the typing mirror keep it a STRING (so the `pattern` applies)?
3. [scenario] The corpus scenario says two errors (blank cert + ws key)
   "collapse to one baseline entry" — is that the harness's actual
   set-equality semantics?
4. [cross-ref] Does the "trimmed-valid stays silent" scenario reflect the
   real validator behavior (unanchored `\S` on `" /etc/certs/crm.pem "`)?

**Answers (with citations):**
1. [glossary] Yes. R-SCHEMA emits `DiagnosticCode::RSchema` at
   `Severity::Error` via `diagnostic_for` (`rschema.rs`), anchored on the
   instance leaf span; the jsonschema `pattern` error's instance path is
   `/mcp/0/server/tls/<field>`, naming the field as the scenarios require.
2. [sharpen] Correct. R-SCHEMA validates the INTERPOLATED copy
   (`interpolated_validation_copy`, rc-93wct) where `${env:X:-d}` resolves
   to its default; `${env:CERT:-}` → `""`. The rc-93wct typing mirror
   forces whole-scalar substituted tokens to JSON STRINGS
   (`rschema.rs` typing-mirror block), so the leaf stays a string and the
   `pattern` keyword applies — `""` fails → Error. Boot substitutes then
   `non_empty_path` rejects — parity. The integer carve-out does not touch
   this string-position leaf.
3. [scenario] Correct. The corpus gate asserts set equality between
   emitted `(code, severity)` pairs and the baseline; "Multiple diagnostics
   sharing the same (code, severity) in one file collapse to a single
   entry" (`lint-corpus-baseline.ron` header; `lint_corpus.rs` `EmittedMap`
   = `BTreeMap<file, BTreeSet<CodeSev>>`). Two `("R-SCHEMA","error")` →
   one baseline tuple.
4. [cross-ref] Confirmed by harness: `" /etc/certs/crm.pem "` →
   `is_match(\S) == true` → no diagnostic; runtime trims and accepts.
   Parity holds.

**Outcome:** confirm — all five scenarios are executable and consistent
with the interpolated-copy validation path, the typing mirror, and the
corpus set-equality semantics. No scenario depends on the mis-spelled
attribute; they assert on the emitted `"pattern": "\\S"` behavior, which is
correct once design FIX 1 is applied.
**Self-grill mode:** self-grill-proposals skill (e_opus for e_gpt;
cross-family verification satisfied).
