# Tasks: envtyped-components

Single-phase change (design.md records no Phases section). Groups below are
crate work packages, not delivery phases.

## camel-config: generic probe helper and provenance store

### Task 1.1: Generalize the probe into `deserialize_block_with_probe` and publish `ProvenanceSet`

**Files:**
- `crates/camel-config/src/env_int_probe.rs` (modified)
- `crates/camel-config/src/lib.rs` (modified)

**Steps:**
1. In `env_int_probe.rs`, make `ProvenanceSet` public: change
   `pub(crate) struct ProvenanceSet(Vec<Vec<ConfigSeg>>)` to `pub struct
   ProvenanceSet(...)` with a doc comment stating (a) it feeds the typed
   probe at deserialize boundaries (env-int-placeholder-typing), (b) paths
   are structural (`ConfigSeg` stays `pub(crate)` — callers never navigate),
   (c) the stale-provenance rule: it describes loader-resolved values; a
   caller that edits `ComponentsConfig.raw` after load must rebuild the
   config so no stale path coerces a token-free value. Keep derives
   `Debug, Clone, Default`; keep `paths()`/`from_paths()` `pub(crate)`.
2. Add `pub(crate) fn split_by_component(&self) ->
   std::collections::HashMap<String, ProvenanceSet>` on `ProvenanceSet`:
   iterate `paths()` in order; a path participates only when it starts with
   `ConfigSeg::Key("components")` followed by `ConfigSeg::Key(block)`; insert
   the remaining segments (possibly empty — skip those) under `block`,
   preserving traversal order.
3. Add the public generic helper in the same file:

   ```rust
   pub fn deserialize_block_with_probe<T, E>(
       block: &toml::Value,
       provenance: &ProvenanceSet,
       parse: impl Fn(toml::Value) -> Result<T, E>,
   ) -> Result<T, E>
   ```

   Body = the existing `deserialize_with_probe` search generalized: first
   pass `parse(block.clone())` (return Ok immediately); on `Err(first_err)`,
   clone `block` ONCE into a `probe_tree` and scan `provenance.paths()`
   through it (`leaf_mut(&mut probe_tree, segs)`, exactly as the current
   code scans `merged_tree` — one clone for the scan, not one per path) —
   candidates are paths whose leaf is `toml::Value::String(s)` with
   `clean_i64(s).map(Integer).or_else(|| clean_bool(s).map(Boolean))`; if
   candidates is empty or `> MAX_PROBE_CANDIDATES`, return `first_err`;
   otherwise retry subsets ascending size lexicographic via the existing
   `next_combination`, each on a fresh `block.clone()` with candidates
   coerced, through `parse`; first `Ok` wins; exhausted → return
   `first_err` verbatim. Doc comment: first pass is the caller's strict
   oracle; probing runs only after failure; cap 8 → at most 255 subset
   retries plus the first pass.
4. Rewrite `deserialize_with_probe(merged_tree, provenance)` as a one-line
   wrapper:
   `deserialize_block_with_probe(merged_tree, provenance, |tree| tree.try_into::<CamelConfig>().map_err(|e| ConfigError::Message(format!("Failed to deserialize merged config: {e}"))))`
   — identical behavior and error text (root-field TOML arm unchanged).
5. In `crates/camel-config/src/lib.rs`, next to the existing `pub use`
   lines, add `pub use env_int_probe::{deserialize_block_with_probe,
   ProvenanceSet};` (module `env_int_probe` stays private).

**Tests:** (in the existing `#[cfg(test)] mod provenance_tests` or a sibling
`probe_helper_tests` module in `env_int_probe.rs`; T types are
test-local `#[derive(Deserialize)]` structs; blocks are VALID TOML documents
parsed with `toml::from_str::<toml::Value>` — top-level assignments, no
braces — whose placeholder values are pre-resolved strings; the helper never
resolves, it probes)
- `helper_first_pass_success_keeps_numeric_string`: fixture document
  `name = "1234"` with provenance path `["name"]`, `T = S { name: String }`,
  parse = `try_into::<S>` → `Ok`, `name == "1234"` (numeric-looking string
  leaf stays a string; no probe ran).
- `helper_coerces_option_u16_leaf`: fixture document
  `fault_status = "503"` (resolved placeholder value, provenance path
  recorded), `T = S { fault_status: Option<u16> }` → `Ok`,
  `fault_status == Some(503)` (spec Option-typed scenario).
- `helper_integer_leaf_coerces`: fixture document `port = "7100"` with
  provenance, `T = S { port: u16 }` → `Ok`, `port == 7100u16`.
- `helper_boolean_leaf_coerces`: fixture document `flag = "true"` with
  provenance, `T = S { flag: Option<bool> }` → `Ok`, `flag == Some(true)`.
- `helper_literal_quoted_numeric_never_coerces`: fixture document
  `port = "8080"` WITHOUT any provenance path, `T =
  std::collections::HashMap<String, u16>` (no test struct) → `Err`, and the
  error is the parse error (no candidate without provenance).
- `helper_cap_nine_candidates_skips_search`: fixture document with nine
  `f0 = "1"` … `f8 = "9"` integer-string assignments and nine matching
  provenance paths, parse target `T = ()` (no test struct — no dead fields
  under `-D warnings`), counting parse closure typed
  `Fn(toml::Value) -> Result<(), &str>` returning `Err("first")` on the
  FIRST call and `Err("retry")` afterward (`AtomicUsize` counter) →
  returned error is `"first"` AND counter `== 1` (cap exceeded before any
  subset retry — asserts the original error is kept, not replaced by the
  retry error).
- `helper_cap_eight_candidates_searches_all_subsets`: same counting parse
  (`Result<(), &str>`, `T = ()`), eight `f0 = "1"` … `f7 = "8"` assignments
  + provenance paths → returned error is `"first"` AND counter `== 256`
  (first pass + all 255 subsets — pins EXHAUSTIVE enumeration at the cap
  boundary; subset ORDER is not asserted here).
- `helper_first_error_returned_verbatim`: fixture document `port = "1"`
  with one provenance path and the same counting parse
  (`Err("first")` first call, `Err("retry")` after, `T = ()`) → returned
  error is `"first"` AND exactly two parse calls happened (first pass + the
  single-subset retry; the ORIGINAL error is returned on no success, never
  the retry error).

**Acceptance:**
- `cargo test -p camel-config --lib` passes with the new tests.
- `cargo clippy -p camel-config --all-targets -- -D warnings` exits 0.
- Existing `deserialize_with_probe` callers compile unchanged
  (`cargo build -p camel-config`).
- No new `unwrap()` outside tests (`cargo xtask lint-unwrap` clean for the
  crate).

- [x] 1.1

### Task 1.2: Store per-block provenance on `ComponentsConfig` and attach at both load entry points

**Files:**
- `crates/camel-config/src/config.rs` (modified)
- `crates/camel-config/src/config_tests/components_provenance_tests.rs` (new)
- `crates/camel-cli/src/commands/job/job_effective_config_tests.rs` (modified)
- `crates/camel-config/tests/placeholder_e2e.rs` (modified)

**Steps:**
1. `ComponentsConfig` (`config.rs:385`): add
   `#[serde(skip)] pub(crate) provenance: HashMap<String, ProvenanceSet>`
   after `raw` (same crate imports the type from
   `crate::env_int_probe`). Remove `PartialEq` from the derive list and
   implement it manually: `fn eq(&self, other: &Self) -> bool {
   self.raw == other.raw }` with a comment — provenance is metadata about how
   values arrived, not a value; derived equality would break
   default-vs-loaded comparisons. Keep `Debug, Clone, Deserialize, Default`
   derived.
2. Add the public cross-crate accessor on `ComponentsConfig`:

   ```rust
   pub fn provenance_for(&self, key: &str) -> &ProvenanceSet
   ```

   returning `self.provenance.get(key)` or a shared empty set (`static
   EMPTY: OnceLock<ProvenanceSet>`). Doc comment: config owns resolution and
   provenance recording; the bundle owns its key and parsing; the accessor
   only hands the block's metadata across that boundary; carries the
   stale-provenance rule from `ProvenanceSet`.
3. Attachment: `load_from_file_inner` itself has no probe line — it
   delegates to `build_from_toml_value_inner`, whose
   `let mut config: CamelConfig = deserialize_with_probe(&merged_tree,
   &provenance)?;` sits at `config.rs:3242`; `from_toml_value_with_env` has
   its own line at `config.rs:2692`. Immediately after EACH of those two
   existing lines, add
   `config.components.provenance = provenance.split_by_component();`
   (before the cache-repo normalization + validate tail, in both functions).
   These two sites cover the sync, async, `_with_env`, and sealed loaders.
   No other construction path changes.
4. Fix the camel-cli test construction site
   (`job_effective_config_tests.rs:51`): replace the struct literal
   `ComponentsConfig { raw: HashMap::from([...]) }` with
   `let mut components = ComponentsConfig::default();
   components.raw = HashMap::from([...]);` used in the enclosing
   `CamelConfig { ... }` expression (cross-crate literals cannot name the
   new private field).

**Tests:**
- `components_provenance_split_by_block` (unit, in
  `env_int_probe.rs` test module): `ProvenanceSet::from_paths(vec![
  [Key("components"), Key("redis"), Key("port")],
  [Key("components"), Key("redis"), Key("sentinel"), Key("port")],
  [Key("components"), Key("ws"), Key("port")],
  [Key("timeout_ms")]])` → `split_by_component()` yields exactly
  `redis → [[port],[sentinel,port]]` and `ws → [[port]]` (order preserved,
  non-component path dropped).
- `provenance_for_empty_default` (unit,
  `config_tests/components_provenance_tests.rs`):
  `ComponentsConfig::default().provenance_for("redis").paths().is_empty()`.
- `components_equality_ignores_provenance` (unit — host in
  `config_tests/components_provenance_tests.rs` too):
  two `ComponentsConfig` with equal `raw`, one with provenance inserted →
  `assert_eq!` holds.
- `component_block_provenance_recorded_and_raw_keeps_strings` (e2e,
  `tests/placeholder_e2e.rs`, using the file's existing `write_main`/`load`
  helpers and `EnvGuard::remove("CFG_REDIS_PORT")`): main file
  `[components.redis]\nport = "${env:CFG_REDIS_PORT:-6379}"\n` → load OK →
  `cfg.components.raw["redis"]["port"].as_str() == Some("6379")` (raw keeps
  the resolved string) AND the provenance is reachable through the PUBLIC
  surface: `camel_config::deserialize_block_with_probe(&raw_redis_value,
  cfg.components.provenance_for("redis"), |v|
  v.try_into::<TestPortConfig>())` with a test-local
  `#[derive(Deserialize)] struct TestPortConfig { port: u16 }` → `Ok`,
  `port == 6379` (integration tests cannot call `ProvenanceSet::paths()` —
  it stays `pub(crate)`; exact-path equality lives in the in-crate test
  below).
- `component_block_provenance_paths_exact` (IN-CRATE — host it in a NEW
  `crates/camel-config/src/config_tests/components_provenance_tests.rs`
  wired with one `#[cfg(test)] #[path = "config_tests/components_provenance_tests.rs"]`
  line alongside the existing includes at `config.rs:1317-1438`; the file
  opens with `use super::*;` like its siblings; keys are real
  `ConfigSeg::Key("port".to_string())` values): build the same tree via
  `CamelConfig::from_toml_value_with_env` with a static lookup (unset var →
  default `"6379"`), then
  `cfg.components.provenance_for("redis").paths() ==
  &[vec![ConfigSeg::Key("port".to_string())]]` (exact structural path,
  traversal order pinned).
- `override_values_never_enter_component_provenance` (e2e,
  `tests/placeholder_e2e.rs`): main file with
  `[components.redis]\nport = "${env:CFG_REDIS_PORT3:-6379}"` — load via
  `load_with_env` (`CamelConfig::from_file_with_env`, the override-merging
  entry; plain `load` keeps the seal and would make this test vacuous) with
  `CAMEL_LOG_LEVEL=warn` set via `EnvGuard::set` (a verified allowlisted
  override) → load OK → `cfg.log_level == "warn"` (the token-free override
  landed on the root field), `raw["redis"]["port"]` stays the resolved
  string `"6379"`, and the public-surface probe roundtrip
  (`deserialize_block_with_probe` + test-local `TestPortConfig`) coerces
  the leaf — the override path feeds root fields only; a component leaf
  never gains provenance from a token-free override (exact-path scoping is
  pinned separately by `component_block_provenance_paths_exact`).
- `root_probe_still_counts_component_candidates` (e2e,
  `tests/placeholder_e2e.rs`): main file
  `timeout_ms = "${env:CFG_ROOT_T:-1500}"` plus an `[components.http]` block
  carrying eight clean placeholder leaves on numeric/boolean `HttpConfig`
  fields (`connect_timeout_ms`, `pool_idle_timeout_ms`, `response_timeout_ms`,
  `read_timeout_ms`, `max_body_size`, `max_response_bytes`,
  `max_request_body`, `follow_redirects` — nine whole-tree candidates); ALL
  process vars guarded/removed under one `EnvGuard` — every `EP_*` var AND
  `CFG_ROOT_T` (an inherited `CFG_ROOT_T=2000` breaks the assertion;
  `CFG_ROOT_T=not-a-number` fails it for the wrong reason; pattern follows
  `placeholder_e2e.rs:358-370`) → `load`
  FAILS with the root first-pass error message
  (`Failed to deserialize merged config` naming `timeout_ms`) — the root
  arm's whole-tree cap is unchanged and bundle parsing is never reached.
  CONTROL test `root_probe_under_cap_still_coerces` in the same file: same
  config minus the `follow_redirects` candidate (one root + seven component
  candidates ≤ cap) → load SUCCEEDS with `timeout_ms == 1500` (makes the
  cap test falsifiable in both directions). The control removes the SAME
  var set (every `EP_*` var AND `CFG_ROOT_T`) under one `EnvGuard`.

**Acceptance:**
- `cargo test -p camel-config` passes (lib + all integration tests,
  including `placeholder_e2e`).
- `cargo test -p camel-cli --lib` passes (updated construction site).
- `cargo clippy -p camel-config -p camel-cli --all-targets -- -D warnings`
  exits 0 — note camel-cli's clippy exclusions in AGENTS.md: for camel-cli
  use the two AGENTS.md clippy invocations
  (`cargo clippy -p camel-cli -- -D warnings` and the
  `--no-default-features --features flavor-regular,exec` variant) since the
  crate is excluded from the workspace clippy set.
- `serde(skip)` confirmed: `toml::from_str::<ComponentsConfig>("[redis]\nport
  = 1")` still parses (existing `components_config_tests` keep passing).

- [x] 1.2

## camel-bundles: the seam

### Task 2.1: Route `bundle_from_config` through the typed probe

**Files:**
- `crates/camel-bundles/src/lib.rs` (modified)

**Steps:**
1. Rewrite the body of `bundle_from_config` (`lib.rs:215`): keep the raw
   lookup + empty-table fallback exactly as today; then

   ```rust
   let provenance = config
       .components
       .provenance_for(<B as camel_component_api::ComponentBundle>::config_key())
       .clone();
   camel_config::deserialize_block_with_probe(&raw, &provenance, |tree| {
       <B as camel_component_api::ComponentBundle>::from_toml(tree)
   })
   ```

   The helper's first pass IS today's `from_toml(raw)`; coercion runs only
   on that failure; the first-pass `CamelError` is returned verbatim when no
   subset parses.
2. Update `bundle_from_config`'s doc comment: state the env-typed behavior
   (provenance-backed clean integer/boolean leaves of THIS block coerce on
   first-pass failure; per-block cap 8; error verbatim; `raw` never
   rewritten), and keep the parse-half-of-`register_bundle` paragraph.
3. No change to `register_bundle`, `register_bundle_with`, `boot`, or any
   bundle crate.

**Tests:** covered by Task 2.2's e2e file; this task's executable check is
the unchanged behavior suite:
- Existing `tests/parity_test.rs` passes unchanged (non-placeholder configs
  take the first pass).
- `cargo test -p camel-bundles` passes.

**Acceptance:**
- `cargo build -p camel-bundles` exits 0.
- `cargo test -p camel-bundles` passes (parity + asset suites unchanged).
- `cargo clippy -p camel-bundles --all-targets -- -D warnings` exits 0.
- Diff touches ONLY `crates/camel-bundles/src/lib.rs`.

- [x] 2.1

### Task 2.2: End-to-end bundle-seam tests for redis, http, ws

**Files:**
- `crates/camel-bundles/tests/envtyped_components_test.rs` (new)

**Steps:**
1. Create the test file with a shared fixture builder:
   `fn config_from(toml_src: &str, lookup: impl Fn(&str) -> Option<String>)
   -> CamelConfig` = `toml::from_str::<toml::Value>(toml_src)` then
   `CamelConfig::from_toml_value_with_env(value, &lookup)` (the public
   embedded seam — resolves placeholders with the injected lookup, attaches
   provenance, validates). Keep fixtures minimal-but-valid: a
   `[components.<key>]` table alone is sufficient (serde defaults cover the
   rest; if `validate()` rejects a bare config, add only the fields the
   validator demands and say so in a comment).
2. Define test-local adapter bundles over the REAL config types (bundles
   keep their parsed configs private; the adapter is the observation path —
   real `config_key`, real typed `try_into` as oracle, exposed parsed
   config):

   ```rust
   struct RedisProbeBundle { config: camel_component_redis::RedisConfig }
   impl ComponentBundle for RedisProbeBundle {
       fn config_key() -> &'static str { "redis" }
       fn from_toml(value: toml::Value) -> Result<Self, CamelError> {
           let config: camel_component_redis::RedisConfig = value.try_into()
               .map_err(|e: toml::de::Error| CamelError::Config(e.to_string()))?;
           Ok(Self { config })
       }
       fn register_all(self, _: &mut dyn ComponentRegistrar) {}
   }
   ```

   and `WsProbeBundle { config: WsConfig }` (key `"ws"`, same shape) and
   `HttpProbeBundle { config: HttpConfig }` (key `"http"` — use the same
   `HttpConfig` the real `HttpBundle::from_toml` parses). The adapters
   mirror the real `from_toml` bodies byte-for-byte (verified against
   `crates/components/camel-redis/src/bundle.rs:30-38`,
   `crates/components/camel-ws/src/bundle.rs`, `crates/components/camel-http/src/bundle.rs`).
   FEATURE GATING (required): `camel-component-redis` and
   `camel-component-ws` are OPTIONAL deps of camel-bundles (on by default),
   so the test file MUST open with
   `#![cfg(all(feature = "redis", feature = "ws"))]` — ungated imports
   break `cargo check -p camel-bundles --tests --no-default-features`.
   The default feature set (the local lane and every acceptance command
   here) enables both, so the tests compile and run there. NO
   dev-dependency additions (the deps exist as optional/default
   dependencies already).
3. Write the tests below. Every error test ALSO calls
   `<Adapter as ComponentBundle>::from_toml(raw_value.clone())` directly and
   asserts the seam error equals the direct error (same variant and payload
   — compare `format!("{e:?}")` or the `CamelError::Config(String)` payload).

**Tests:** (all in `crates/camel-bundles/tests/envtyped_components_test.rs`;
names exact; each line is setup → action → assertion)
- `redis_port_via_env_value_coerces`: fixture
  `[components.redis]\nport = "${env:EP_REDIS_PORT}"`; lookup
  `EP_REDIS_PORT = "7100"` → `bundle_from_config::<RedisProbeBundle>(&cfg)`
  → `Ok`, `bundle.config.port == 7100`.
- `redis_port_default_coerces`: fixture
  `[components.redis]\nport = "${env:EP_REDIS_PORT:-6379}"`, lookup without
  `EP_REDIS_PORT` → `Ok`, `bundle.config.port == 6379`.
- `ws_boolean_leaf_coerces`: fixture
  `[components.ws]\nbinary_payload = "${env:EP_WS_BIN:-true}"`, lookup
  without `EP_WS_BIN` → `bundle_from_config::<WsProbeBundle>` → `Ok`,
  `bundle.config.binary_payload == Some(true)`.
- `numeric_string_leaf_stays_string`: fixture
  `[components.redis]\nhost = "${env:EP_REDIS_HOST:-1234}"`, lookup without
  the var → `Ok` on the FIRST pass, `bundle.config.host == "1234"` (String
  field keeps the numeric-looking string; no coercion ran).
- `literal_quoted_numeric_stays_rejected`: fixture
  `[components.ws]\nmax_connections = "8080"` →
  `bundle_from_config::<WsProbeBundle>` Err, and the error equals the direct
  `WsProbeBundle::from_toml(raw)` error byte-for-byte (invalid type: string).
- `unrelated_error_surfaces_verbatim`: fixture
  `[components.redis]\nport = "${env:EP_REDIS_PORT2:-6379}"\nconnection_timeout_secs = "not-a-number"`
  → Err, equal (variant + payload) to the direct `from_toml` error.
- `two_blocks_each_coerce`: fixture with
  `[components.redis]\nport = "${env:EP_P1}"` and
  `[components.http]\nconnect_timeout_ms = "${env:EP_P2:-5000}"` (HttpConfig
  has no `port` field; `connect_timeout_ms` is a real u64 leaf); lookup
  both (`EP_P1 = "7100"`, `EP_P2 = "8080"`) →
  `bundle_from_config::<RedisProbeBundle>` → `port == 7100` AND
  `bundle_from_config::<HttpProbeBundle>` → `connect_timeout_ms == 8080`
  (per-block scoping).
- `nine_split_across_blocks_both_parse`: redis block with four clean
  placeholder candidates on its numeric/boolean `RedisConfig` fields —
  `port = "${env:EP_S1:-7100}"`, `connection_timeout_secs =
  "${env:EP_S2:-17}"`, `tls = "${env:EP_S3:-true}"`, `tls_mode =
  "${env:EP_S4:-false}"` — and http block with five (`connect_timeout_ms =
  "${env:EP_S5:-201}"`, `pool_idle_timeout_ms = "${env:EP_S6:-202}"`,
  `response_timeout_ms = "${env:EP_S7:-203}"`, `follow_redirects =
  "${env:EP_S8:-true}"`, `max_body_size = "${env:EP_S9:-204}"` — real
  `HttpConfig` fields); ALL lookups unset (values come from the defaults)
  → both adapters parse `Ok` with the EXACT values asserted: redis
  `port == 7100`, `connection_timeout_secs == 17`, `tls == true`,
  `tls_mode == Some(false)`; http `connect_timeout_ms == 201`,
  `pool_idle_timeout_ms == 202`, `response_timeout_ms == 203`,
  `follow_redirects == true`, `max_body_size == 204` (per-block cap is
  per-block; nine total > 8).
- `nine_candidates_one_block_keeps_first_error`: http block with NINE clean
  placeholder candidates in placeholder defaults, lookups unset —
  `connect_timeout_ms = "${env:EP_N1:-101}"`, `pool_max_idle_per_host =
  "${env:EP_N2:-102}"`, `pool_idle_timeout_ms = "${env:EP_N3:-103}"`,
  `response_timeout_ms = "${env:EP_N4:-104}"`, `read_timeout_ms =
  "${env:EP_N5:-105}"`, `max_body_size = "${env:EP_N6:-106}"`,
  `max_response_bytes = "${env:EP_N7:-107}"`, `max_request_body =
  "${env:EP_N8:-108}"`, `follow_redirects = "${env:EP_N9:-true}"` — and NO
  other defect (an otherwise-coercible block: with the cap removed the
  full-coercion subset would parse) →
  `bundle_from_config::<HttpProbeBundle>` Err, equal (variant + payload) to
  the direct `from_toml` error (cap exceeded before any subset search).
- `eight_candidates_one_block_still_searches`: the nine list minus
  `follow_redirects`, defaults `101`–`108` in the listed order, lookups
  unset, no other defect → `Ok` with the EXACT values asserted
  (`connect_timeout_ms == 101`, `pool_max_idle_per_host == 102`,
  `pool_idle_timeout_ms == 103`, `response_timeout_ms == 104`,
  `read_timeout_ms == 105`, `max_body_size == 106`, `max_response_bytes ==
  107`, `max_request_body == 108`) — cap boundary searches.

**Acceptance:**
- `cargo test -p camel-bundles --test envtyped_components_test` passes.
- `cargo test -p camel-bundles` passes (whole crate, local lane row).
- `cargo clippy -p camel-bundles --all-targets -- -D warnings` exits 0.
- The test file constructs NO production getters and modifies NO bundle
  crate — observation is via the adapters only.

- [x] 2.2

## Docs and lane registration

### Task 3.1: Document the component-block rule and add the camel-config lane

**Files:**
- `docs/src/configuration/env-interpolation.md` (modified)
- `AGENTS.md` (modified)

**Steps:**
1. In `docs/src/configuration/env-interpolation.md`, add a `## Component
   configuration` section after `## Typed values` (keeping the doc's
   existing heading style): a `[components.redis]` TOML example with
   `port = "${env:REDIS_PORT:-6379}"`; the typed-leaf rule — integer and
   boolean leaves of a `[components.*]` block coerce at the bundle seam when
   the bundle's own parse fails, with the same clean-value gates as root
   fields and a cap of eight candidates per component block after a
   successful root load (the root probe still counts the whole tree);
   `components.raw` keeps the RESOLVED STRINGS (resolution happens at config
   load; coercion never rewrites `raw`); the stale-provenance rule — a
   caller that edits `ComponentsConfig.raw` after load must rebuild the
   config; and the boundary note that route-document/DSL seams are covered
   separately (rc-3c67o.2).
2. In `AGENTS.md ## COMPONENT LANES`, add to the local rows (next to the
   other per-crate `local` rows):

   ```
   # --- camel-config: own tests/ + integration surface (mission 603)
   camel-config | local | cargo test -p camel-config
   ```

3. Verify the existing `camel-bundles | local | cargo test -p camel-bundles`
   row (AGENTS.md:178) runs as part of the sweep — no edit needed to it.

Notes: prose register for the new doc section is plain technical English
matching the file's existing style (ste-writing applies to OpenSpec
artifacts, not this doc).

**Tests:**
- `cd docs && mdbook test` passes (the new section's TOML snippets must not
  break doc tests; plain fenced toml blocks without language annotation are
  inert — match the file's existing snippet style).
- Lane parser acceptance:
  `GATES_ONLY=lanes bash scripts/gates-local.sh` runs the new camel-config
  row (visible in the sweep output) and exits 0.

**Acceptance:**
- `cd docs && mdbook test` exits 0.
- `bash scripts/gates-local.sh` exits 0 with the camel-config lane present.

- [x] 3.1
