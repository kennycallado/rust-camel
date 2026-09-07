# Tasks: env-int-placeholder-parity (rev 2: wave-E canon)

Bd: rc-93wct. Rev 2 after wave-E collision (e_glm verdict D, see bd). Premise flip: interpolation typing follows the tree-walk canon (numeric-looking leaves keep STRING typing) — int-position placeholders are load/lint Errors everywhere (boot parity); string-position placeholders interpolate. Single phase. All commands run from the worktree root.

## Task 1.1 — camel-dsl: tree-walk-first loader + shared `interpolate_yaml_source` seam

**Files:**
- `crates/camel-dsl/src/env_interpolation.rs` (modified — new pub seam; module doc SYNC note already present)
- `crates/camel-dsl/src/yaml.rs` (modified)
- `crates/camel-dsl/src/discovery.rs` (modified — YAML arm delegates to the seam)
- `crates/camel-dsl/CONTEXT.md` (modified — glossary entry re-worded to tree-walk-first)

**Steps:**
1. In `env_interpolation.rs` add `pub fn interpolate_yaml_source(raw: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Result<String, String>` mirroring discovery's `interpolate_for_parse` YAML arm exactly: `interpolate_env_tree(raw, lookup)` → Ok; `Err(TreeInterpolateError::Unresolved(var))` → `Err(var)`; `Err(TreeInterpolateError::Fallback)` → `interpolate_env_with(raw, lookup)`.
2. Refactor `discovery.rs` `interpolate_for_parse`'s `Some("yaml") | Some("yml")` arm to call `interpolate_yaml_source` (single source; JSON arm unchanged). Zero behavior change.
3. `yaml.rs` `load_from_file_with_env`: replace the direct `interpolate_env_with` call with `interpolate_yaml_source(&content, lookup)`; keep the existing `CamelError::RouteError("Environment variable '{var}' not set (required by {path})")` mapping for `Err(var)`.
4. Update `crates/camel-dsl/CONTEXT.md` glossary entry "file route loading (env-interpolated)": tree-walk-first semantics, string typing of substituted leaves, legacy fallback, injectable lookup.

**Tests** (rewrite in `yaml.rs` `mod tests`; tempfile pattern):
- `load_from_file_interpolates_string_placeholder` — `title: ${env:RC_T:-hello}` (string-typed field on a step that has one) → Ok, value `"hello"`.
- `load_from_file_int_placeholder_fails_boot_parity` — `throttle.max_requests: ${env:MY_LIMIT:-2}` → Err (route fails; same class discovery produces).
- `load_from_file_unset_no_default_errors_named` — `title: ${env:UNSET_NO_DEF_xyz}` → Err contains `UNSET_NO_DEF_xyz` + lowercase `not set`, NOT a serde "did not match" error.
- `load_from_file_ambient_env_ignored_by_default_path` — ambient `RC93WCT_VAR=goodbye` set/removed (unsafe set_var pattern) → resolves `"hello"`.
- `load_from_file_with_env_injection_point` — lookup `Some("goodbye")` → `"goodbye"`.
- `load_from_file_commented_no_default_placeholder_harmless` — comment `# ${env:RC_C}` + literal body → Ok (tree-walk never touches comments).
- `load_from_file_roundtrip_fragile_fails_with_parse_error_not_env_error` — a tagged-node route file with a resolvable string placeholder → Err, the document's own parse/deserialization error (no env-var wording).
- Discovery parity guard: `discovery_and_load_from_file_agree_on_int_placeholder` — same int-placeholder temp file: `discover_routes` errs AND `load_from_file` errs (both rejected).
- command: `cargo test -p camel-dsl --lib load_from_file` + `cargo test -p camel-dsl --lib discovery` — new tests red before steps 1-3 (int/typing flips), green after.

**Acceptance:**
- Both test commands exit 0; `cargo test -p camel-dsl --lib` green overall.
- `cargo fmt --check`, `cargo clippy -p camel-dsl -- -D warnings` exit 0.

- [x] 1.1

## Task 1.2 — camel-cli: LEAN inline branch on the shared seam; tests flipped to boot parity

**Files:**
- `crates/camel-cli/src/commands/test/runner.rs` (modified)
- `crates/camel-cli/src/commands/test/driver_tests.rs` (modified — tests)

**Steps:**
1. Inline branch of `load_routes` (~runner.rs:189-194): replace `interpolate_env_with(&text, &|_| None)` with `camel_dsl::interpolate_yaml_source(&text, &|_| None)` (root re-export via camel-dsl lib.rs if needed); keep the `Environment variable '{var}' not set (required by inline routes)` doc-error mapping.
2. Update the load_routes doc comment: file forms and inline both use the tree-walk-first loader semantics (boot parity).
3. File branches unchanged (Task 1.1 loader carries the strategy).

**Tests** (driver_tests.rs via `runner::load_routes`; #[tokio::test] precedent):
- `lean_file_route_string_placeholder_loads` — route file with string-typed field `${env:LEAN_T:-hello}` via `routeFiles` → Ok, interpolated value.
- `lean_file_route_int_placeholder_doc_error` — `circuit_breaker.open_duration_ms: ${env:CB_MS:-750}` → returned Err (route load failure — boot parity; assert the error mentions the route file path or step class, NOT the named-var wording).
- `lean_inline_routes_string_placeholder_loads` — inline `routes:` with string field `${env:P:-one}` → Ok.
- `lean_unset_no_default_doc_error_names_var` — `title: ${env:LEAN_UNDEF_xyz}` → Err contains `LEAN_UNDEF_xyz` + lowercase `not set`.
- `lean_ignores_ambient_env` — ambient `LEAN_T=ambient` set/removed → `"hello"`.

**Acceptance:**
- `cargo test -p camel-cli lean_` exit 0; `cargo fmt --check`; `cargo clippy -p camel-cli -- -D warnings` exit 0.

- [x] 1.2

## Task 1.3 — camel-lint: typing mirror (whole-scalar tokens validate as strings)

**Files:**
- `crates/camel-lint/src/env_interpolation.rs` (modified)
- `crates/camel-lint/src/rules/rschema.rs` (modified)
- `crates/camel-lint/CONTEXT.md` (modified — paragraph re-worded)

**Steps:**
1. Extend the validation copy: after building the interpolated instance, detect value leaves whose AUTHORED scalar (from the ORIGINAL CST/`doc.raw`, quotes/whitespace trimmed) is EXACTLY one substituted `${env:X:-d}` token (whole-scalar; embedded-in-larger-string tokens are NOT whole-scalar) and force the corresponding instance leaf to the JSON STRING `"d"` (replicating tree-walk typing; numeric/boolean re-inference suppressed).
2. Whole-scalar NO-default tokens (`${env:X}`, unescaped): force nothing (leave literal) AND emit one Error-severity `DiagnosticCode::RSchema` anchored on the token (boot hard-fails on them; `$${env:...}` escapes exempt — no diagnostic).
3. Keep: Info note per substituted default (string positions now the happy path), per-token semantics, span machinery (occurrence queues from rev 1), `Span::new(0,0)` miss behavior, comment tokens produce NO diagnostics (skip tokens with no resolvable value-leaf span — comments yield none).
4. CONTEXT.md: typing-mirror paragraph (int/bool positions Error; string positions Info; escapes; parity rationale).

**Tests** (camel-lint unit tests):
- `rschema_string_position_default_silent_with_info` — `title: ${env:MY_TITLE:-hello}` → zero Errors, one Info (message + span slice assertions).
- `rschema_int_position_default_type_error` — `throttle.max_requests: ${env:MY_LIMIT:-2}` → one Error anchored on the placeholder (validated leaf is string `"2"`).
- `rschema_no_default_flagged_at_string_position` — `title: ${env:NO_DEF_xyz}` → Error flagging the token; `$${env:X}` escape → no diagnostic.
- `rschema_mixed_document_per_token` — string-position WITH_DEF (Info) AND int-position ALSO_DEF (Error) in one doc.
- `rschema_commented_tokens_no_diagnostics` — comments with `${env:X}` and `${env:Y:-d}` → zero diagnostics from them.
- `rschema_numeric_default_in_string_field_stays_string` — string-typed field `title: ${env:T:-8080}` → NO type error (latent rev-1 false positive now fixed), one Info.
- Keep rev-1 tests that still hold: hermeticity witness, duplicate-token spans (string positions), mirror parity table.

**Acceptance:**
- `cargo test -p camel-lint` exit 0; `rg -n 'std::env|env::var' crates/camel-lint/src` zero hits; fmt/clippy clean.

- [x] 1.3

## Task 1.4 — corpus, LSP, docs re-baseline under typing canon

**Files:**
- `crates/camel-cli/tests/fixtures/lint-corpus/env-int-placeholder.yaml` (modified — fixture becomes string-position placeholder happy case)
- `crates/camel-cli/tests/fixtures/lint-corpus-baseline.ron` (modified — re-derive from a fresh run)
- `crates/camel-lsp/tests/lsp_session.rs` (modified — session test flips to string-position doc)
- `crates/camel-dsl/README.md` (verify still accurate)
- `crates/camel-cli/CONTEXT.md` (verify LEAN paragraph matches boot-parity semantics)

**Steps:**
1. Fixture: switch the placeholder to a string-typed field (`title: ${env:RC93WCT_TITLE:-demo}`) — clean route, expected exactly one Info entry.
2. Run corpus test; re-derive the baseline honestly — pre-existing files (examples/env-interpolation/routes/routes.yaml, consumer.yaml) must keep exactly one `("R-SCHEMA","info")` each IF their tokens are string-position (they boot fine post-E, so they must be); report any surprise.
3. LSP session test `placeholder_int_field_publishes_info_not_error`: rename to `placeholder_string_field_publishes_info_not_error` (or keep name if it still reads honestly — prefer rename); document uses a string-position placeholder; assertions: no ERROR anywhere, one INFORMATION naming the token var.
4. Verify README/CONTEXT claims match rev-2 (tree-walk-first, string typing).

**Acceptance:**
- `cargo test -p camel-cli --test lint_corpus` exit 0; `cargo test -p camel-lsp` exit 0; `cargo fmt --check` clean.

- [x] 1.4
