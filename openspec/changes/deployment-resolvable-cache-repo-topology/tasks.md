# Tasks: deployment-resolvable-cache-repo-topology

## camel-config

### Task 1.1: Normalize empty redis topology values to absent (FR1)

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Add free fn `fn normalize_empty_topology_fields(url: &mut Option<String>, sentinel_nodes: &mut Option<Vec<String>>, master_name: &mut Option<String>, username: &mut Option<String>, sentinel_username: &mut Option<String>, key_prefix: &mut Option<String>)` near `validate_redis_topology_fields` (:871) — flat `&mut Option` args mirroring that fn's style, with a doc comment stating the shared contract (prevents the two repos drifting, same rationale as the validator's doc). Rules: each `Option<String>` — `Some(s)` where `s.trim().is_empty()` becomes `None`; `sentinel_nodes` — `Some(v)` where `v.is_empty() || v.iter().all(|n| n.trim().is_empty())` becomes `None`. `password` and `sentinel_password` join as parameters (blank → `None`, non-blank never dropped); `db` is untouched (typed, no empty form). [amended post-implementation: blank credentials mean unset]
2. Add thin inherent method `CacheRepoConfig::normalize_empty_topology(&mut self)` in the existing `impl CacheRepoConfig` block (~:800) delegating to the free fn with `&mut` borrows of its six fields.
3. Add thin inherent method `IdempotentRepoConfig::normalize_empty_topology(&mut self)` (impl block near its struct, ~:528) delegating identically.
4. In `build_from_toml_value_inner` (:2443), immediately after `CamelConfig` deserialization (~:2591) and before `.validate()` (~:2594), insert — GATED on backend, because unconditional normalization would legitimize `url = ""` on memory/redb sections that cross-backend validation rejects today (:1841, :1977): `if let Some(repo) = config.cache_repo.as_mut() && repo.backend == "redis" { repo.normalize_empty_topology(); }` and the same backend-gated call for the `idempotent_repo` field (:33).
5. Do NOT modify `validate_redis_topology_fields` — after normalization its `is_some()` predicates see clean `None`s.

**Tests:** (executable spec — name, setup, action, assert; all pipeline tests hold `ENV_OVERRIDE_LOCK` for every env var they set/unset, including `RC_TEST_*`, per the rule documented at :2788-2798)
- `normalize_blank_string_topology_fields_to_none`: build `CacheRepoConfig` with `url: Some("".into())`, `master_name: Some("   ".into())`, `username: Some("".into())`, `sentinel_username: Some("\t".into())`, `key_prefix: Some("".into())`, `sentinel_nodes: Some(vec![])`, `password: Some("x".into())`, `sentinel_password: Some("".into())` → call `normalize_empty_topology()` → assert the six topology fields are `None`, `sentinel_password` is `None`, `password` still `Some("x")` (non-blank credential preserved). [amended: blank credentials normalize to unset]
- `normalize_all_blank_sentinel_array_to_none`: `sentinel_nodes: Some(vec![" ".into(), "".into()])` → normalize → `None`.
- `mixed_blank_sentinel_array_not_normalized`: `sentinel_nodes: Some(vec!["redis-a:26379".into(), " ".into()])` → normalize → still `Some` with both entries; then build a `CacheRepoConfig` (backend "redis", that nodes value, valid `master_name`) → `config.validate()` errors with the existing non-empty-entry sentinel message.
- `idempotent_repo_normalize_parity`: unit — `IdempotentRepoConfig` with `url: Some("".into())` + populated `sentinel_nodes`/`master_name` → normalize → `url == None`; pipeline — tempdir TOML with `[idempotent_repo] backend="redis"`, `url="${env:RC_TEST_IDEM_URL:-}"` (unset), `sentinel_nodes` + `master_name` populated → load → `validate()` Ok, `url == None`.
- `blank_key_prefix_selects_default`: pipeline — valid standalone section with `key_prefix="${env:RC_TEST_PREFIX:-}"` unset → load → validate Ok, `key_prefix == None`; unit — `key_prefix: Some("bad*prefix")` → normalize → stays `Some("bad*prefix")` (non-blank invalid values are not normalized; keyspace validation still rejects them).
- `sentinel_topology_selected_by_empty_expanded_url` (spec scenario 1): tempdir TOML with `[cache_repo] backend="redis"`, `url="${env:RC_TEST_REDIS_URL:-}"`, `sentinel_nodes=["node-a:26379"]`, `master_name="m"`; `RC_TEST_REDIS_URL` unset → load via `CamelConfig::from_file` → assert `config.validate()` is Ok and `cache_repo.url` is `None`.
- `standalone_topology_selected_by_populated_url` (spec scenario 2): same file but with `RC_TEST_REDIS_URL=redis://host:6379` set, `sentinel_nodes=["${env:RC_TEST_NODES_0:-}"]`, `master_name="${env:RC_TEST_MASTER:-}"` both unset → load → validate Ok, `sentinel_nodes`/`master_name` are `None`.
- `literal_empty_url_in_file_treated_as_unset` (spec scenario 5): `url = ""` literal + `sentinel_nodes` populated → load → validate Ok as sentinel.
- `both_topologies_absent_still_fails` (spec scenario 6): `url="${env:RC_TEST_MISSING:-}"`, `sentinel_nodes` key absent → `CamelConfig::from_file(...)` itself returns the "requires a topology" validation error (the loader validates internally).
- `memory_backend_empty_url_still_rejected` (backend-gate regression): `[cache_repo] backend="memory"`, `url=""` → load fails with the existing cross-backend rejection (`url does not apply to the "memory"` backend).
- `redb_backend_empty_url_still_rejected` (backend-gate regression): `[cache_repo] backend="redb"`, `path="/tmp/x.redb"`, `cache_size="64MiB"`, `url=""` → load fails with the redb cross-backend rejection. Same for an `[idempotent_repo] backend="redb"` fixture with `url=""`.
- `command`: `cargo test -p camel-config --lib` — all new tests listed above appear and pass. RED-ability: the PIPELINE tests fail if step 4's call or backend gate is removed or the rules are inverted; the UNIT normalization tests fail only if step 1-3 logic is inverted (they do not exercise step 4).

**Acceptance:**
- `cargo test -p camel-config --lib` exits 0 with all new tests passing.
- `cargo clippy -p camel-config --all-features -- -D warnings` exits 0.
- `rg -n "normalize_empty_topology" crates/camel-config/src/config.rs` shows the free fn, both wrappers, and the two pipeline call sites by name.

### Task 1.2: Non-credential cache_repo env overrides + CSV coercion (FR2)

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Append 8 entries to `ALLOWED_ENV_OVERRIDES` (:2363): `CAMEL_CACHE_REPO_PAYLOAD`, `CAMEL_CACHE_REPO_PAYLOAD_DIR`, `CAMEL_CACHE_REPO_CACHE_SIZE`, `CAMEL_CACHE_REPO_SWEEP_INTERVAL`, `CAMEL_CACHE_REPO_MASTER_NAME`, `CAMEL_CACHE_REPO_KEY_PREFIX`, `CAMEL_CACHE_REPO_DB`, `CAMEL_CACHE_REPO_SENTINEL_NODES`.
2. Add `const CSV_ENV_OVERRIDES: &[&str] = &["CAMEL_CACHE_REPO_SENTINEL_NODES"];` and `const EMPTY_SCALAR_ENV_OVERRIDES: &[&str] = &["CAMEL_CACHE_REPO_PAYLOAD", "CAMEL_CACHE_REPO_PAYLOAD_DIR", "CAMEL_CACHE_REPO_CACHE_SIZE", "CAMEL_CACHE_REPO_SWEEP_INTERVAL", "CAMEL_CACHE_REPO_MASTER_NAME", "CAMEL_CACHE_REPO_KEY_PREFIX", "CAMEL_CACHE_REPO_DB"];` next to the allowlist with doc comments: CSV const — only list-typed override, entries split on `,`, trimmed, blanks dropped, empty input yields an empty array; empty-scalar const — the ONLY vars for which an empty raw value is skipped (scoped so pre-existing allowlisted vars keep their exact current behavior, e.g. `CAMEL_TIMEOUT_MS=""` still fails typed deserialization loudly).
3. Add `fn parse_env_csv_list(val: &str) -> Vec<String>` near `parse_env_value` (:2336): split on `,`, `trim()` each entry, keep only non-empty entries, return the Vec.
4. In the env-merge loop (:2529-2569, `parse_env_value` call at :2536), replace the unconditional parse with a three-way branch: (a) var in `EMPTY_SCALAR_ENV_OVERRIDES` and raw value `is_empty()` → `continue` (skip; file/profile value preserved — empty string must never reach `Option<u16>` deserialization); (b) var in `CSV_ENV_OVERRIDES` → produce `serde_json::Value::Array(entries.into_iter().map(serde_json::Value::String).collect())` via `parse_env_csv_list` (empty input → empty array) — the loop builds a `serde_json::Map`; the existing `cache_repo_` prefix branch (:2558) maps `SENTINEL_NODES` to the `sentinel_nodes` field and array values overwrite the file list through the JSON→toml→`Option<Vec<String>>` chain; (c) otherwise → `parse_env_value` as today (pre-existing vars unchanged).
5. Do NOT add `CAMEL_CACHE_REPO_URL`, `_USERNAME`, `_PASSWORD`, `_SENTINEL_USERNAME`, `_SENTINEL_PASSWORD` anywhere in the allowlist (L-C2). The existing "env var not in config override allowlist; ignored" warning path (:2525) stays unchanged for them.

**Tests:** (executable spec; every test holds `ENV_OVERRIDE_LOCK` and uses `CamelConfig::from_file_with_env` — plain `from_file` passes `merge_env=false` and overrides never merge; sentinel fixtures include `master_name` because `db` applies only in sentinel mode, config.rs:944-948)
- `scalar_override_db_applied` (spec scenario 9): file `[cache_repo] backend="redis"`, `sentinel_nodes=["n:26379"]`, `master_name="m"`, `db=0`; set `CAMEL_CACHE_REPO_DB=3` → load → assert `cache_repo.db == Some(3)` and validate Ok.
- `empty_scalar_override_preserves_file_value` (spec scenario 10): same fixture with `db=5`; set `CAMEL_CACHE_REPO_DB=` (empty) → load → assert `db == Some(5)` and load did not error.
- `csv_override_builds_trimmed_node_list` (spec scenario 11): file `[cache_repo] backend="redis"`, `master_name="m"`, no `sentinel_nodes`; set `CAMEL_CACHE_REPO_SENTINEL_NODES="node-a:26379, node-b:26379"` → load → assert `sentinel_nodes == Some(vec!["node-a:26379","node-b:26379"])` and validate Ok.
- `csv_override_plus_master_name_validates_sentinel` (spec scenario 12): file with `backend="redis"`, no `url`, no `sentinel_nodes`, `master_name="${env:RC_TEST_MASTER:-}"`; env: `CAMEL_CACHE_REPO_SENTINEL_NODES="node-a:26379,node-b:26379"` and `RC_TEST_MASTER=mymaster` → load → `validate()` Ok, master_name expanded to "mymaster".
- `empty_csv_override_clears_populated_file_value` (spec scenario 13): file with `[cache_repo] backend="redis"`, `sentinel_nodes=["file-node:26379"]`, standalone `url="redis://s:6379"`, no `master_name` and no sentinel credentials; env `CAMEL_CACHE_REPO_SENTINEL_NODES=` empty → load → assert `sentinel_nodes == None` (override replaced the file list with `[]`, Task 1.1 normalization applied) and `validate()` Ok as standalone.
- `credential_vars_stay_denied` (spec scenario 14): file `[cache_repo] backend="redis"`, `sentinel_nodes=["n:26379"]`, `master_name="m"`; set `CAMEL_CACHE_REPO_URL=redis://evil:6379` and `CAMEL_CACHE_REPO_USERNAME=attacker` → load → assert loaded `url` is `None` (file has none), `username` unchanged, and via `log_capture::capture_warns` that SEPARATE captured records contain the structured `var="CAMEL_CACHE_REPO_URL"` / `var="CAMEL_CACHE_REPO_USERNAME"` fields (match the actual structured field name in the warn call at :2525) and each record contains the fragment "env var not in config override allowlist; ignored".
- `allowlist_completeness_pinned`: unit — assert all 8 new vars are present in `ALLOWED_ENV_OVERRIDES` and all 5 credential vars (`CAMEL_CACHE_REPO_URL`, `_USERNAME`, `_PASSWORD`, `_SENTINEL_USERNAME`, `_SENTINEL_PASSWORD`) are absent.
- `empty_preexisting_typed_override_still_fails`: regression — set `CAMEL_TIMEOUT_MS=` (empty; pre-existing allowlisted var, NOT in `EMPTY_SCALAR_ENV_OVERRIDES`) with a minimal valid file → load fails with the typed-deserialization error it produces today (behavior preserved, not silently skipped).
- `command`: `cargo test -p camel-config --lib` — all listed tests pass; RED against a build without steps 1–4.

**Acceptance:**
- `cargo test -p camel-config --lib` exits 0.
- `cargo clippy -p camel-config --all-features -- -D warnings` exits 0.
- `rg -n "CAMEL_CACHE_REPO_(URL|USERNAME|PASSWORD|SENTINEL_USERNAME|SENTINEL_PASSWORD)" crates/camel-config/src/config.rs` returns hits ONLY in tests (the allowlist itself contains none of them).

### Task 1.3: Operator docs for the new override surface

**Files:**
- `docs/src/configuration/index.md` (modified)

**Steps:**
1. ADD a new subsection "Environment overrides" to the configuration chapter (no `CAMEL_*` override section exists today — only a passing mention at index.md:3). Content: the 8 new `CAMEL_CACHE_REPO_*` vars (table or list), the CSV format for `SENTINEL_NODES`, the empty-scalar-preserves / empty-CSV-clears rule, and a note that connection strings and credentials are only settable via `${env:}` placeholders, never env overrides.
2. Cross-link the empty-means-unset rule to the per-profile complete `cache_repo` pattern documented for bd rc-btbn (same file).

**Tests:** (non-Rust — doc verification; each check separate so a missing var fails individually)
- `docs_mention_each_new_var`: `rg -q "CAMEL_CACHE_REPO_SENTINEL_NODES" docs/src/configuration/index.md` AND `rg -q "CAMEL_CACHE_REPO_MASTER_NAME" ...` AND `rg -q "CAMEL_CACHE_REPO_DB" ...` AND `rg -q "CAMEL_CACHE_REPO_KEY_PREFIX" ...` AND `rg -q "CAMEL_CACHE_REPO_PAYLOAD" ...` AND `rg -q "CAMEL_CACHE_REPO_CACHE_SIZE" ...` AND `rg -q "CAMEL_CACHE_REPO_SWEEP_INTERVAL" ...` AND `rg -q "CAMEL_CACHE_REPO_PAYLOAD_DIR" ...` — all 8 present.
- `docs_state_empty_semantics`: `rg -qi "empty.*preserv" docs/src/configuration/index.md` (empty-scalar preserves file value) AND `rg -qi "clears" docs/src/configuration/index.md` (empty-CSV clears).
- `command`: `mdbook build docs` exits 0.

**Acceptance:**
- `mdbook build docs` exits 0.
- Both `rg` checks above return within bounds.
- Prose matches STE conventions (short sentences, active voice, English).

## Checklist

- [x] 1.1 — normalize empty redis topology to absent (FR1, rc-5kyu) — commit 7d3da468
- [x] 1.2 — non-credential cache_repo env overrides + CSV (FR2, rc-2o9e) — commit 5b0f9f06
- [x] 1.3 — operator docs, environment overrides chapter — commit 48c56d18
