//! Redb-backed persistent idempotent repository.
//!
//! # Schema
//!
//! - Table `idempotent_keys`: `&str → ()`  (presence = key was added)

use std::fmt;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use camel_api::{CamelError, IdempotentRepository};
use redb::{Durability, ReadableDatabase, ReadableTable, TableDefinition};

use crate::JournalDurability;

// ── Table definition ──────────────────────────────────────────────────────────

/// Membership table. A stored row means "this key is present in the
/// idempotent set"; the unit value carries no payload.
const KEYS_TABLE: TableDefinition<&str, ()> = TableDefinition::new("idempotent_keys");

// ── Repository ────────────────────────────────────────────────────────────────

/// Redb-backed implementation of `IdempotentRepository`.
///
/// `Arc<Database>` allows cheap cloning — all clones share the same
/// underlying redb file handle. `redb::Database` is `Send + Sync`.
pub struct RedbIdempotentRepository {
    name: String,
    path: PathBuf,
    /// Shared redb database handle. `Arc<Database>` is `Send + Sync` and clones
    /// cheaply so each `spawn_blocking` closure can take its own reference.
    db: Arc<redb::Database>,
    durability: JournalDurability,
}

impl RedbIdempotentRepository {
    /// Open (or create) the redb database at `path` and initialise the
    /// `idempotent_keys` table on first open.
    ///
    /// Parent directories are created if they do not exist. The whole
    /// open sequence is offloaded to `tokio::task::spawn_blocking` because
    /// `redb::Database::create` is blocking.
    pub async fn new(
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        durability: JournalDurability,
    ) -> Result<Self, CamelError> {
        let name = name.into();
        let path: PathBuf = path.into();
        let path_for_db = path.clone();
        let db = tokio::task::spawn_blocking(move || {
            if let Some(parent) = path_for_db.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| CamelError::Io(format!("redb create_dir_all: {e}")))?;
            }
            let db = redb::Database::create(&path_for_db)
                .map_err(|e| CamelError::Io(format!("redb open: {e}")))?;
            let wtx = db
                .begin_write()
                .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
            wtx.open_table(KEYS_TABLE)
                .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
            wtx.commit()
                .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
            Ok::<_, CamelError>(db)
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))??;

        Ok(Self {
            name,
            path,
            db: Arc::new(db),
            durability,
        })
    }

    /// Map our domain `JournalDurability` onto redb's `Durability` knob.
    /// Mirrors `RedbRuntimeEventJournal::redb_durability` so write-txn
    /// durability is consistent across the runtime.
    fn redb_durability(&self) -> Durability {
        match self.durability {
            JournalDurability::Immediate => Durability::Immediate,
            JournalDurability::Eventual => Durability::None,
        }
    }
}

// ── IdempotentRepository impl ─────────────────────────────────────────────────

#[async_trait]
impl IdempotentRepository for RedbIdempotentRepository {
    fn name(&self) -> &str {
        &self.name
    }

    async fn contains(&self, key: &str) -> Result<bool, CamelError> {
        let db = Arc::clone(&self.db);
        let key = key.to_string();
        tokio::task::spawn_blocking(move || {
            let rtx = db
                .begin_read()
                .map_err(|e| CamelError::Io(format!("redb begin_read: {e}")))?;
            let table = rtx
                .open_table(KEYS_TABLE)
                .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
            table
                .get(key.as_str())
                .map_err(|e| CamelError::Io(format!("redb get: {e}")))
                .map(|opt| opt.is_some())
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))?
    }

    async fn add(&self, key: &str) -> Result<bool, CamelError> {
        let db = Arc::clone(&self.db);
        let key = key.to_string();
        let durability = self.redb_durability();
        tokio::task::spawn_blocking(move || {
            let mut wtx = db
                .begin_write()
                .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
            wtx.set_durability(durability)
                .map_err(|e| CamelError::Io(format!("redb set_durability: {e}")))?;
            // The write table borrows `wtx`, so it must be dropped before
            // `wtx.commit()` is called.
            let was_new = {
                let mut table = wtx
                    .open_table(KEYS_TABLE)
                    .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
                let prior = table
                    .insert(key.as_str(), ())
                    .map_err(|e| CamelError::Io(format!("redb insert: {e}")))?;
                prior.is_none()
            };
            wtx.commit()
                .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
            Ok(was_new)
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))?
    }

    async fn remove(&self, key: &str) -> Result<(), CamelError> {
        let db = Arc::clone(&self.db);
        let key = key.to_string();
        let durability = self.redb_durability();
        tokio::task::spawn_blocking(move || {
            let mut wtx = db
                .begin_write()
                .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
            wtx.set_durability(durability)
                .map_err(|e| CamelError::Io(format!("redb set_durability: {e}")))?;
            {
                let mut table = wtx
                    .open_table(KEYS_TABLE)
                    .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
                // The returned `Option` is intentionally ignored — `remove` is
                // idempotent per the trait contract.
                let _ = table
                    .remove(key.as_str())
                    .map_err(|e| CamelError::Io(format!("redb remove: {e}")))?;
            }
            wtx.commit()
                .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))?
    }

    async fn clear(&self) -> Result<(), CamelError> {
        let db = Arc::clone(&self.db);
        let durability = self.redb_durability();
        tokio::task::spawn_blocking(move || {
            let mut wtx = db
                .begin_write()
                .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
            wtx.set_durability(durability)
                .map_err(|e| CamelError::Io(format!("redb set_durability: {e}")))?;
            {
                let mut table = wtx
                    .open_table(KEYS_TABLE)
                    .map_err(|e| CamelError::Io(format!("redb open_table: {e}")))?;
                // Collect keys into a Vec first, then drop the iterator before
                // calling `remove` — `table.iter()` borrows the table mutably
                // and would conflict with `remove` otherwise.
                let keys: Vec<String> = table
                    .iter()
                    .map_err(|e| CamelError::Io(format!("redb iter: {e}")))?
                    .map(|r| r.map(|(k, _v)| k.value().to_string()))
                    .collect::<Result<_, _>>()
                    .map_err(|e| CamelError::Io(format!("redb iter item: {e}")))?;
                for k in &keys {
                    let _ = table
                        .remove(k.as_str())
                        .map_err(|e| CamelError::Io(format!("redb remove: {e}")))?;
                }
            }
            wtx.commit()
                .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))?
    }
}

impl fmt::Debug for RedbIdempotentRepository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RedbIdempotentRepository")
            .field("name", &self.name)
            .field("path", &self.path)
            .field("durability", &self.durability)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use camel_api::{
        CamelError, Exchange, IdempotentRepository, Message, OutcomePipeline, OutcomeSegment,
        PipelineOutcome, Value,
    };
    use camel_processor::{IdempotentConsumerSegment, MessageIdExpression};
    use tempfile::{TempDir, tempdir};

    use crate::JournalDurability;
    use crate::idempotent::RedbIdempotentRepository;

    /// Open a repo at `<tmp>/idempotent.redb` with `Immediate` durability.
    async fn new_repo(tmp: &TempDir) -> RedbIdempotentRepository {
        let path = tmp.path().join("idempotent.redb");
        RedbIdempotentRepository::new("redb", path, JournalDurability::Immediate)
            .await
            .expect("open redb repo")
    }

    #[tokio::test]
    async fn redb_repo_construct_opens_database_and_creates_parent() {
        let dir = tempdir().expect("tempdir");
        let path = dir
            .path()
            .join("nested")
            .join("dir")
            .join("idempotent.redb");
        // Parent does not exist yet — `new` must create it.
        assert!(!path.parent().expect("parent").exists());

        let result =
            RedbIdempotentRepository::new("redb", path.clone(), JournalDurability::Immediate).await;

        assert!(result.is_ok(), "expected Ok, got {result:?}");
        assert!(path.exists(), "redb file must exist after construct");
    }

    #[tokio::test]
    async fn redb_repo_construct_fails_when_parent_is_a_regular_file() {
        let dir = tempdir().expect("tempdir");
        let blocker = dir.path().join("blocker");
        // A regular file, not a directory — `create_dir_all` must fail.
        std::fs::write(&blocker, b"not a dir").expect("write blocker");
        let path = blocker.join("idempotent.redb");

        let result =
            RedbIdempotentRepository::new("redb", path, JournalDurability::Immediate).await;

        assert!(
            matches!(result, Err(CamelError::Io(_))),
            "expected Err(CamelError::Io(_)), got {result:?}"
        );
    }

    #[tokio::test]
    async fn redb_repo_debug_impl_does_not_require_database_debug() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("idempotent.redb");
        let repo = RedbIdempotentRepository::new("redb", path, JournalDurability::Immediate)
            .await
            .expect("construct");

        let dbg = format!("{repo:?}");
        assert!(
            dbg.contains("RedbIdempotentRepository"),
            "Debug output must contain struct name, got: {dbg}"
        );
        assert!(
            dbg.contains("redb"),
            "Debug output must contain name, got: {dbg}"
        );
    }

    #[tokio::test]
    async fn redb_repo_add_new_key_returns_true_duplicate_returns_false() {
        let dir = tempdir().expect("tempdir");
        let repo = new_repo(&dir).await;

        let first = repo.add("msg-1").await.expect("first add");
        let second = repo.add("msg-1").await.expect("second add");

        assert!(first, "first add of new key must return Ok(true)");
        assert!(!second, "second add of same key must return Ok(false)");
    }

    #[tokio::test]
    async fn redb_repo_contains_reflects_add_and_remove() {
        let dir = tempdir().expect("tempdir");
        let repo = new_repo(&dir).await;
        repo.add("msg-1").await.expect("setup add");

        let before = repo.contains("msg-1").await.expect("contains before");
        repo.remove("msg-1").await.expect("remove");
        let after = repo.contains("msg-1").await.expect("contains after");

        assert!(before, "contains must report present after add");
        assert!(!after, "contains must report absent after remove");
    }

    #[tokio::test]
    async fn redb_repo_clear_removes_all_keys() {
        let dir = tempdir().expect("tempdir");
        let repo = new_repo(&dir).await;
        for k in ["a", "b", "c"] {
            repo.add(k).await.expect("setup add");
        }

        repo.clear().await.expect("clear");

        for k in ["a", "b", "c"] {
            let present = repo.contains(k).await.expect("contains after clear");
            assert!(!present, "key {k} must be gone after clear");
        }
    }

    #[tokio::test]
    async fn redb_repo_keys_persist_across_reopened_handle() {
        let dir = tempdir().expect("tempdir");
        {
            let repo_a = new_repo(&dir).await;
            repo_a.add("msg-1").await.expect("setup add");
            // repo_a dropped here, closing the underlying redb file handle.
        }
        let repo_b = new_repo(&dir).await;
        let present = repo_b
            .contains("msg-1")
            .await
            .expect("contains after reopen");
        assert!(
            present,
            "key added by repo A must be visible to repo B after reopen"
        );
    }

    #[tokio::test]
    async fn redb_repo_concurrent_add_same_key_yields_one_success() {
        let dir = tempdir().expect("tempdir");
        let repo = new_repo(&dir).await;

        let (a, b) = tokio::join!(repo.add("k"), repo.add("k"));

        // `CamelError` does not implement `PartialEq`, so use `.as_ref().is_ok_and`
        // to extract the success value rather than `Result == Ok(...)`. Semantics
        // are identical to the spec's `(a == Ok(true)) ^ (b == Ok(true))`.
        // Exactly one writer must observe the slot as empty.
        assert!(
            a.as_ref().is_ok_and(|v| *v) ^ b.as_ref().is_ok_and(|v| *v),
            "exactly one branch must report Ok(true); got a={a:?}, b={b:?}"
        );
        // The other branch must report Ok(false) (already present).
        assert!(
            a.as_ref().is_ok_and(|v| !*v) ^ b.as_ref().is_ok_and(|v| !*v),
            "the other branch must report Ok(false); got a={a:?}, b={b:?}"
        );
    }

    #[tokio::test]
    async fn redb_repo_eventual_durability_commits_without_fsync() {
        let dir = tempdir().expect("tempdir");
        let path = dir.path().join("idempotent.redb");
        let repo = RedbIdempotentRepository::new("redb", path, JournalDurability::Eventual)
            .await
            .expect("open eventual repo");

        let added = repo.add("x").await.expect("add with eventual durability");
        assert!(added, "eventual-durability add must succeed for a new key");
    }

    // ── Apache-Camel-IdempotentRepository contract hardening ──

    /// `clear()` on an empty repository must succeed (no panic, no error).
    /// Mirrors Apache Camel's `AbstractIdempotentRepositoryTest.clearIsEmpty`
    /// contract edge: clear-when-empty is a valid no-op.
    #[tokio::test]
    async fn redb_repo_clear_on_empty_returns_ok() {
        let dir = tempdir().expect("tempdir");
        let repo = new_repo(&dir).await;

        let result = repo.clear().await;
        assert!(
            result.is_ok(),
            "clear on an empty repo must succeed, got {result:?}"
        );
    }

    /// `remove()` of a key that was never added must succeed (idempotent remove).
    /// The `IdempotentRepository` trait contract states: "Succeeds even if the
    /// key does not exist." Verifies post-condition: the key remains absent.
    #[tokio::test]
    async fn redb_repo_remove_nonexistent_key_returns_ok() {
        let dir = tempdir().expect("tempdir");
        let repo = new_repo(&dir).await;

        let result = repo.remove("ghost").await;
        assert!(
            result.is_ok(),
            "remove of absent key must succeed, got {result:?}"
        );
        let present = repo
            .contains("ghost")
            .await
            .expect("contains after remove-of-absent");
        assert!(
            !present,
            "absent key must remain absent after a no-op remove"
        );
    }

    // ── Test helpers for the end-to-end dedup test below ──

    /// `ScriptedChild` records a single invocation and returns a configurable
    /// `PipelineOutcome`. Mirrors the helper of the same name in
    /// `camel-processor::idempotent_consumer::tests`, but scoped to this
    /// test module so the test does not depend on private types.
    struct ScriptedChild {
        outcome: PipelineOutcome,
        invoked: Arc<AtomicBool>,
    }

    impl OutcomePipeline for ScriptedChild {
        fn clone_box(&self) -> Box<dyn OutcomePipeline> {
            // The test never clones the segment that owns this child.
            unreachable!("clone_box not used in redb_repo dedup test")
        }

        fn run<'a>(
            &'a mut self,
            exchange: Exchange,
        ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
            self.invoked
                .store(true, std::sync::atomic::Ordering::SeqCst);
            let outcome = std::mem::replace(
                &mut self.outcome,
                PipelineOutcome::Completed(Exchange::new(Message::new(""))),
            );
            Box::pin(async move { outcome_with_exchange(outcome, exchange) })
        }
    }

    /// Replace the placeholder exchange inside a scripted outcome with the
    /// real exchange passed to `run`. Keeps the variant, swaps the payload.
    fn outcome_with_exchange(outcome: PipelineOutcome, exchange: Exchange) -> PipelineOutcome {
        match outcome {
            PipelineOutcome::Completed(_) => PipelineOutcome::Completed(exchange),
            PipelineOutcome::Stopped(_) => PipelineOutcome::Stopped(exchange),
            PipelineOutcome::Failed(e) => PipelineOutcome::Failed(e),
        }
    }

    /// Key extractor that reads the `id` header and returns its string value.
    fn header_id_extractor() -> MessageIdExpression {
        Arc::new(|ex: &Exchange| {
            ex.input
                .header("id")
                .and_then(|v| v.as_str().map(|s| s.to_string()))
        })
    }

    /// Build a non-eager `IdempotentConsumerSegment` with the supplied repo
    /// and a `ScriptedChild` returning `child_outcome`. Returns the segment
    /// plus a shared `AtomicBool` that flips to `true` iff the child runs.
    fn build_redb_segment(
        repo: Arc<dyn IdempotentRepository>,
        child_outcome: PipelineOutcome,
    ) -> (IdempotentConsumerSegment, Arc<AtomicBool>) {
        let invoked = Arc::new(AtomicBool::new(false));
        let child = ScriptedChild {
            outcome: child_outcome,
            invoked: invoked.clone(),
        };
        let segment = IdempotentConsumerSegment::new(
            repo,
            header_id_extractor(),
            OutcomeSegment::new(Box::new(child)),
            false, // non-eager: key added after child Completed
            false, // remove_on_failure: keep key even if child fails
        );
        (segment, invoked)
    }

    fn exchange_with_header_id(id: &str) -> Exchange {
        let mut ex = Exchange::new(Message::new("payload"));
        ex.input.set_header("id", Value::String(id.into()));
        ex
    }

    // ── End-to-end dedup test (Apache Camel `FileIdempotentConsumerTest`) ──
    // Proves the redb repository actually deduplicates when driven by the
    // `IdempotentConsumerSegment`, with a real persistent backend and the
    // real outcome pipeline — not a `MockRepo`.
    #[tokio::test]
    async fn redb_repo_drives_idempotent_consumer_dedup_e2e() {
        let dir = tempdir().expect("tempdir");
        let repo: Arc<dyn IdempotentRepository> = Arc::new(new_repo(&dir).await);

        // Run #1: new key "X" → child MUST run, repo MUST record "X".
        let (mut seg1, invoked1) = build_redb_segment(
            repo.clone(),
            PipelineOutcome::Completed(Exchange::new(Message::new(""))),
        );
        let outcome1 = seg1.run(exchange_with_header_id("X")).await;
        assert!(
            matches!(outcome1, PipelineOutcome::Completed(_)),
            "first X delivery must produce Completed, got {outcome1:?}"
        );
        assert!(
            invoked1.load(std::sync::atomic::Ordering::SeqCst),
            "child MUST run for new key X"
        );
        assert!(
            repo.contains("X").await.expect("contains X after run 1"),
            "X must be present in redb after first successful run"
        );

        // Run #2: duplicate "X" → child MUST NOT run; segment short-circuits.
        // Configure the child to return Failed so any accidental invocation
        // is doubly visible (the outcome assertion below also fails).
        let (mut seg2, invoked2) = build_redb_segment(
            repo.clone(),
            PipelineOutcome::Failed(CamelError::ProcessorError(
                "duplicate must not reach the child".into(),
            )),
        );
        let outcome2 = seg2.run(exchange_with_header_id("X")).await;
        assert!(
            matches!(outcome2, PipelineOutcome::Completed(_)),
            "duplicate X MUST short-circuit to Completed, got {outcome2:?}"
        );
        assert!(
            !invoked2.load(std::sync::atomic::Ordering::SeqCst),
            "child MUST NOT run for duplicate X (idempotent filter)"
        );

        // Run #3: new key "Y" → child MUST run, repo MUST record "Y".
        let (mut seg3, invoked3) = build_redb_segment(
            repo.clone(),
            PipelineOutcome::Completed(Exchange::new(Message::new(""))),
        );
        let outcome3 = seg3.run(exchange_with_header_id("Y")).await;
        assert!(
            matches!(outcome3, PipelineOutcome::Completed(_)),
            "first Y delivery must produce Completed, got {outcome3:?}"
        );
        assert!(
            invoked3.load(std::sync::atomic::Ordering::SeqCst),
            "child MUST run for new key Y"
        );
        assert!(
            repo.contains("Y").await.expect("contains Y after run 3"),
            "Y must be present in redb after third successful run"
        );
        // X must still be present from run #1 (clear on dup did not happen).
        assert!(
            repo.contains("X").await.expect("contains X after run 3"),
            "X must still be present after duplicate was filtered"
        );
    }
}
