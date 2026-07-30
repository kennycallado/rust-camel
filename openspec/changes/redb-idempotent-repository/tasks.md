# Tasks: redb-idempotent-repository

Single-phase change. Grouped by module. Each task is independently dispatchable; tasks run in the listed order because later tasks reference symbols introduced earlier.

## camel-core/src/idempotent

### Task 1.1: RedbIdempotentRepository struct, constructor, and manual Debug impl

**Files:**
- `crates/camel-core/src/idempotent/redb_repository.rs` (new)
- `crates/camel-core/src/idempotent/mod.rs` (modified — add module + re-export)
- `crates/camel-core/src/lib.rs` (modified — add crate-root re-export next to the `RedbRuntimeEventJournal` block at lines 122-124)

**Steps:**
1. Create `crates/camel-core/src/idempotent/redb_repository.rs`. Define the membership table with a unit value (the journal proves `()` is a valid redb value — see `COMMAND_IDS_TABLE: TableDefinition<&str, ()>` at `redb_journal.rs:31`): `const KEYS_TABLE: redb::TableDefinition<&str, ()> = redb::TableDefinition::new("idempotent_keys");`. A stored row means "key present"; the value carries no payload. This matches `design.md` exactly.
2. Define the struct: `pub struct RedbIdempotentRepository { name: String, path: std::path::PathBuf, db: std::sync::Arc<redb::Database>, durability: crate::JournalDurability }`. The field MUST be `Arc<redb::Database>`, NOT a bare `redb::Database` — a bare `Database` is NOT `Clone`, and the trait methods must share one handle into `spawn_blocking` (the journal stores `db: Arc<Database>` at `redb_journal.rs:85` for exactly this reason). Do NOT derive `Debug` — neither `redb::Database` nor `Arc<Database>` is `Debug`.
3. Implement a constructor `pub async fn new(name: impl Into<String>, path: impl Into<std::path::PathBuf>, durability: crate::JournalDurability) -> Result<Self, camel_api::CamelError>` that: (a) converts `name`/`path` to owned values and clones them into a `move` closure; (b) runs `tokio::task::spawn_blocking` whose closure performs steps (c)-(f); (c) inside the closure, call `std::fs::create_dir_all(parent)` where `parent` is `path.parent()` (skip if parent is `None`, e.g. relative file in cwd); (d) call `redb::Database::create(&path).map_err(|e| CamelError::Io(format!("redb open: {e}")))?`; (e) open/create the table: `let wtx = db.begin_write().map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?; wtx.open_table(KEYS_TABLE).map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?; wtx.commit().map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;`; (f) return the `RedbIdempotentRepository`, storing the db as `db: std::sync::Arc::new(db)` (mirror `redb_journal.rs:131` — the struct holds `Arc<Database>` per step 2 because a bare `redb::Database` is not `Clone`). Map the `spawn_blocking` join error to `CamelError::Io(format!("spawn_blocking join: {e}"))`. Follow the exact error-mapping form used in `crates/camel-core/src/lifecycle/adapters/redb_journal.rs` (`begin_write`/`commit` lines).
4. Implement `std::fmt::Debug` for `RedbIdempotentRepository` by hand: write a `DebugStruct` named `"RedbIdempotentRepository"` with fields `.field("name", &self.name).field("path", &self.path).field("durability", &self.durability)`. Do not touch `self.db`.
5. In `crates/camel-core/src/idempotent/mod.rs` add `pub mod redb_repository;` and `pub use redb_repository::RedbIdempotentRepository;` next to the existing `memory_repository` lines.
6. In `crates/camel-core/src/lib.rs` add a SEPARATE feature-gated re-export statement (do NOT insert into the existing `redb_journal` brace block at lines 122-125, which lists `JournalDurability, JournalEntry, JournalInspectFilter, RedbJournalOptions, RedbRuntimeEventJournal` — appending a `crate::idempotent::RedbIdempotentRepository` entry there would put a different module path inside the journal use-list and fail to compile). Add its own standalone statement: `#[cfg(feature = "export-internal-adapters")] pub use crate::idempotent::RedbIdempotentRepository;`. camel-config already enables `export-internal-adapters` (it reaches the gated `RedbRuntimeEventJournal` today), so `camel_core::RedbIdempotentRepository` resolves there. (The `pub mod idempotent;` at lib.rs:68 is already ungated, so the full path `camel_core::idempotent::RedbIdempotentRepository` also resolves regardless of the feature.)

**Tests:** (executable spec — `#[tokio::test]`, use `tempfile::TempDir` which is already a camel-core dev-dependency used by the redb journal tests)
- `redb_repo_construct_opens_database_and_creates_parent`: setup = a `TempDir`, a path `<tmp>/nested/dir/idempotent.redb` whose parent does not exist, durability `JournalDurability::Immediate`; action = `RedbIdempotentRepository::new("redb", path.clone(), JournalDurability::Immediate).await`; assert = `Ok` and the file `path` exists on disk afterward.
- `redb_repo_construct_fails_when_parent_is_a_regular_file`: setup = create a `TempDir`, write a regular file at `<tmp>/blocker` (a file, not a dir), then attempt to build a repo at `<tmp>/blocker/idempotent.redb`; action = `RedbIdempotentRepository::new("redb", path, JournalDurability::Immediate).await`; assert = `Err(CamelError::Io(..))` (Construction-failure C1 scenario). Use `assert!(matches!(res, Err(CamelError::Io(_))))`.
- `redb_repo_debug_impl_does_not_require_database_debug`: setup = construct a repo on a temp path; action = `format!("{:?}", repo)`; assert = the resulting string contains `"RedbIdempotentRepository"` and the repo name `"redb"`.

**Acceptance:**
- `cargo check -p camel-core` exits 0.
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo test -p camel-core --lib redb_repo` passes all three tests above.
- `cargo fmt --check --all` exits 0.

- [x] 1.1

### Task 1.2: IdempotentRepository trait impl (contains/add/remove/clear)

**Files:**
- `crates/camel-core/src/idempotent/redb_repository.rs` (modified — add `#[async_trait::async_trait] impl camel_api::IdempotentRepository for RedbIdempotentRepository`)

**Steps:**
The impl block MUST be annotated `#[async_trait::async_trait]` — the trait is defined with `#[async_trait]` (the compiling stub at `camel-api/src/idempotent.rs:51` and `MemoryIdempotentRepository` confirm this). Do NOT attempt a native `async fn` impl.
1. `impl crate::JournalDurability` → redb mapping helper `fn redb_durability(&self) -> redb::Durability` mirroring `redb_journal.rs` line ~194: `Immediate => redb::Durability::Immediate`, `Eventual => redb::Durability::None`.
2. `fn name(&self) -> &str` returns `&self.name`.
3. `async fn contains(&self, key: &str) -> Result<bool, CamelError>`: take `let db = std::sync::Arc::clone(&self.db);` (share the one `Arc<Database>` handle — a bare `redb::Database` is NOT `Clone`; the journal stores `Arc<Database>` at `redb_journal.rs:85`), `key.to_string()`, then `spawn_blocking(move || { let rtx = db.begin_read().map_err(|e| CamelError::Io(format!("redb begin_read: {e}")))?; let table = rtx.open_table(KEYS_TABLE).map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?; Ok(table.get(key.as_str())?.is_some()) })` and flatten the join error to `CamelError::Io(format!("spawn_blocking join: {e}"))`.
4. `async fn add(&self, key: &str) -> Result<bool, CamelError>`: `let db = std::sync::Arc::clone(&self.db);`, `key.to_string()`, `self.redb_durability()`, then `spawn_blocking(move || { let mut wtx = db.begin_write().map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?; wtx.set_durability(durability).map_err(|e| CamelError::Io(format!("redb set_durability: {e}")))?; let mut table = wtx.open_table(KEYS_TABLE).map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?; let prior = table.insert(key.as_str(), ()).map_err(|e| CamelError::Io(format!("redb insert: {e}")))?; wtx.commit().map_err(|e| CamelError::Io(format!("redb commit: {e}")))?; Ok(prior.is_none()) })`. `set_durability` returns `Result` in redb v4 and MUST be mapped (the journal does so at `redb_journal.rs:334-335`); leaving it unhandled trips `unused_must_use` under `clippy -D warnings`. The `()` value matches `KEYS_TABLE`'s value type. Flatten join error.
5. `async fn remove(&self, key: &str) -> Result<(), CamelError>`: same write-txn shape; call `table.remove(key.as_str()).map_err(|e| CamelError::Io(format!("redb remove: {e}")))?;` (ignore the returned `Option`), then `wtx.commit()`.
6. `async fn clear(&self) -> Result<(), CamelError>`: `let db = std::sync::Arc::clone(&self.db);` + durability, `spawn_blocking(move || { let mut wtx = db.begin_write().map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?; wtx.set_durability(durability).map_err(|e| CamelError::Io(format!("redb set_durability: {e}")))?; let mut table = wtx.open_table(KEYS_TABLE).map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?; let keys: Vec<String> = table.iter().map_err(|e| CamelError::Io(format!("redb iter: {e}")))?.map(|r| r.map(|(k, _v)| k.value().to_string())).collect::<Result<_, _>>().map_err(|e| CamelError::Io(format!("redb iter item: {e}")))?; for k in &keys { table.remove(k.as_str()).map_err(|e| CamelError::Io(format!("redb remove: {e}")))?; } wtx.commit().map_err(|e| CamelError::Io(format!("redb commit: {e}")))?; Ok(()) })`. Collecting keys into a `Vec` before `remove` avoids holding the iterator borrow while mutating.
7. Every method must map BOTH the redb error and the `spawn_blocking` join error to `CamelError::Io(..)` — never unwrap, never panic (the project's `lint-unwrap` gate forbids `unwrap()`).

**Tests:** (executable spec — `#[tokio::test]`, `tempfile::TempDir`; helper `async fn new_repo(tmp: &TempDir) -> RedbIdempotentRepository` opening `<tmp>/idempotent.redb`)
- `redb_repo_add_new_key_returns_true_duplicate_returns_false`: setup = `new_repo`; action = `add("msg-1").await` then `add("msg-1").await`; assert = `Ok(true)` then `Ok(false)`.
- `redb_repo_contains_reflects_add_and_remove`: setup = `new_repo` with `"msg-1"` already added; action = `contains("msg-1")`, then `remove("msg-1")`, then `contains("msg-1")`; assert = first `Ok(true)`, `remove` `Ok(())`, second `Ok(false)`.
- `redb_repo_clear_removes_all_keys`: setup = `new_repo` with `"a"`, `"b"`, `"c"` added; action = `clear().await`, then `contains("a")`, `contains("b")`, `contains("c")`; assert = `clear` `Ok(())`, all three `contains` `Ok(false)`.
- `redb_repo_keys_persist_across_reopened_handle`: setup = repo A opened on `<tmp>/idempotent.redb` that has added `"msg-1"`; action = drop A, open repo B on the same path, `B.contains("msg-1").await`; assert = `Ok(true)`.
- `redb_repo_concurrent_add_same_key_yields_one_success`: setup = `new_repo`; action = `tokio::join!(repo.add("k"), repo.add("k"))`; assert = exactly one branch is `Ok(true)` and the other `Ok(false)` (assert `(a == Ok(true)) ^ (b == Ok(true))`).
- `redb_repo_eventual_durability_commits_without_fsync`: setup = `new_repo_with(durability = JournalDurability::Eventual)`; action = `add("x").await`; assert = `Ok(true)` (verifies the Eventual write path commits; this is a smoke test for the durability branch, not an fsync syscall check).

**Acceptance:**
- `cargo clippy -p camel-core -- -D warnings` exits 0.
- `cargo test -p camel-core --lib redb_repo_` passes all six tests above.
- `cargo xtask lint-unwrap` introduces no new `unwrap()`/`expect()` in `redb_repository.rs`.
- `cargo fmt --check --all` exits 0.

- [x] 1.2

## camel-config

### Task 2.1: RedbIdempotentConfig type, CamelConfig field, and validation

**Files:**
- `crates/camel-config/src/config.rs` (modified)

**Steps:**
1. Add a new config struct near the existing `JournalConfig` (around line 461). REUSE the existing `JournalDurability` enum (already defined in this file around line 433 with `Immediate`/`Eventual` and an existing `From` to `camel_core::JournalDurability`) instead of creating a duplicate — both backends have identical durability semantics, so a second enum would be byte-for-byte noise. Define:
   ```rust
   #[derive(Debug, Clone, Deserialize, PartialEq)]
   #[serde(deny_unknown_fields)]
   pub struct RedbIdempotentConfig {
       /// Path to the `.redb` file. Created if it does not exist.
       pub path: std::path::PathBuf,
       /// Durability mode. Default: `immediate`.
       #[serde(default)]
       pub durability: JournalDurability,
   }
   ```
   Note: this intentionally reuses `JournalDurability` rather than the design's proposed mirror `IdempotentDurability` enum — DRY; the spec only pins the field name `idempotent_repo`, not the durability type.
2. Add a `pub idempotent_repo: Option<RedbIdempotentConfig>` field to `CamelConfig` (the top-level config struct that already carries `runtime_journal: Option<JournalConfig>`). Do NOT add `#[serde(default)]` — `Option` fields are already absent-by-default in serde, so the attribute would be redundant (clippy/`deny_unknown_fields` agree). Place it adjacent to `runtime_journal` and update the `Default` impl for `CamelConfig` if one exists (default to `None`).
3. Add a config-validation rule mirroring the existing `JournalConfig` empty-path validator (around line 1050 / `test_config_empty_journal_path_rejected`): if `idempotent_repo` is `Some` and its `path` is empty, validation returns an error naming `idempotent_repo.path`. Reuse whatever validation mechanism the journal config uses (the same validator type/function pattern).
4. Document the durability trade-off in a doc comment on `RedbIdempotentConfig`: `Immediate` fsyncs per added key (correctness parity); high-throughput routes set `Eventual` and accept at-least-once degradation on OS/power crash.

**Tests:** (executable spec — unit tests in `config.rs`'s `#[cfg(test)]` module, mirroring `redb_journal_options_from_journal_config_copies_fields` and `test_config_empty_journal_path_rejected`)
- `redb_idempotent_config_defaults_to_immediate_durability`: setup = parse a TOML/serde fragment `[idempotent_repo]\npath = "x.redb"` (no durability); action = deserialize into `CamelConfig`; assert = `config.idempotent_repo.unwrap().durability == JournalDurability::Immediate`.
- `redb_idempotent_config_parses_eventual_durability`: setup = parse a TOML/serde fragment `[idempotent_repo]\npath = "x.redb"\ndurability = "eventual"`; action = deserialize into `CamelConfig`; assert = `config.idempotent_repo.unwrap().durability == JournalDurability::Eventual`.
- `redb_idempotent_config_empty_path_rejected`: setup = a config fragment with `path = ""`; action = run the config validator; assert = returns an error whose message contains `idempotent_repo` and `path`.
- `redb_idempotent_config_durability_roundtrips_to_core`: setup = `RedbIdempotentConfig { path: std::path::PathBuf::from("x.redb"), durability: JournalDurability::Eventual }`; action = `camel_core::JournalDurability::from(cfg.durability)`; assert = equals `camel_core::JournalDurability::Eventual`.

**Acceptance:**
- `cargo check -p camel-config` exits 0.
- `cargo clippy -p camel-config --all-features -- -D warnings` exits 0.
- `cargo test -p camel-config --lib redb_idempotent_config_` passes all four tests above.
- `cargo xtask schema --check` exits 0. If it reports a stale checked-in schema artifact for the config (the new `idempotent_repo` field changed the shape), regenerate it with the matching `cargo xtask schema` command (no `--check`) and commit the regenerated artifact. If camel-config has no JSON-schema generation, the gate is a no-op and passes unchanged.
- `cargo fmt --check --all` exits 0.

- [x] 2.1

### Task 2.2: Wire redb idempotent repository into context_ext.rs

**Files:**
- `crates/camel-config/src/context_ext.rs` (modified)

**Steps:**
1. The registration MUST happen on the BUILT context, not the builder — `CamelContextBuilder` has NO `register_idempotent_repository` method (only `CamelContext` does, at `context.rs:826`, taking `&mut self`). The registry is an `Arc<Mutex<HashMap>>` (`registry.rs:34`) shared with `DefaultRouteController` via an `Arc::clone` taken during `build()`, and `register(&self)` uses interior mutability, so registering AFTER `build()` is still visible to the controller's later name resolution. In `configure_context` (`context_ext.rs`), immediately AFTER the `let mut ctx = builder.build().await?;` line (~line 215), add: `if let Some(ref icfg) = config.idempotent_repo { let durability = camel_core::JournalDurability::from(icfg.durability.clone()); let repo = camel_core::RedbIdempotentRepository::new("redb", icfg.path.clone(), durability).await?; ctx.register_idempotent_repository("redb", Arc::new(repo)).map_err(|e| CamelError::Config(format!("register idempotent 'redb': {e:?}")))?; }`. There is NO `From<RegistryError> for CamelError`, so map the `RegistryError` explicitly to `CamelError::Config(..)`. Do NOT use `unwrap`/`expect` (`lint-unwrap` forbids them). Do NOT place this in the pre-`build()` journal-wiring block (~line 160-166) — `builder` has no register method and the registry does not exist there yet.
2. Do NOT change the default `"memory"` registration (it stays in `CamelContextBuilder::build()` at `context_builder.rs:212`); this branch only adds `"redb"` when configured.
3. Add a one-line doc comment on the branch stating redb is opt-in and `"memory"` remains the default.

**Tests:** (executable spec — integration tests in `crates/camel-config/tests/` mirroring `context_config_test.rs`)
- `context_registers_redb_idempotent_when_configured`: setup = a `CamelConfig` with `idempotent_repo = Some(RedbIdempotentConfig { path: <tmp>/idempotent.redb, durability: Immediate })`; action = build the context from config; assert = `ctx.idempotent_repository("redb").is_some()` AND `ctx.idempotent_repository("memory").is_some()`.
- `context_redb_absent_when_not_configured_memory_remains_default`: setup = a `CamelConfig` with `idempotent_repo = None`; action = build the context; assert = `ctx.idempotent_repository("redb").is_none()` AND `ctx.idempotent_repository("memory").is_some()`.

**Acceptance:**
- `cargo clippy -p camel-config --all-features -- -D warnings` exits 0.
- `cargo test -p camel-config --test context_config_test` (or the new test file) passes both tests above.
- `cargo build --workspace` exits 0.
- `cargo fmt --check --all` exits 0.

- [x] 2.2
