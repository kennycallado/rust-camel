//! Redb-backed runtime event journal.
//!
//! # Schema
//!
//! - Table `events`: `u64 → &[u8]`  (seq → serde_json bytes of `JournalEntry`)
//! - Table `command_ids`: `&str → ()`  (presence = alive)
//!
//! # Sequence numbers
//!
//! No autoincrement in redb. Each `append_batch` derives `next_seq` by reading
//! the last key via `iter().next_back()` and adding 1; defaults to 0 if empty.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use serde::{Deserialize, Serialize};

use camel_api::CamelError;

use crate::lifecycle::domain::{DomainError, RuntimeEvent};
use crate::lifecycle::ports::RuntimeEventJournalPort;

// ── Table definitions ─────────────────────────────────────────────────────────

const EVENTS_TABLE: TableDefinition<u64, &[u8]> = TableDefinition::new("events");
const COMMAND_IDS_TABLE: TableDefinition<&str, ()> = TableDefinition::new("command_ids");

// ── Public types ──────────────────────────────────────────────────────────────

/// Durability mode for journal writes.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum JournalDurability {
    /// fsync on every commit — protects against kernel crash and power loss (default).
    #[default]
    Immediate,
    /// No fsync — OS decides flush timing. Suitable for dev/test.
    Eventual,
}

/// Options for `RedbRuntimeEventJournal`.
#[derive(Debug, Clone)]
pub struct RedbJournalOptions {
    pub durability: JournalDurability,
    /// Trigger compaction after this many events in the table. Default: 10_000.
    pub compaction_threshold_events: u64,
}

impl Default for RedbJournalOptions {
    fn default() -> Self {
        Self {
            durability: JournalDurability::Immediate,
            compaction_threshold_events: 10_000,
        }
    }
}

/// Internal wire format stored as redb value bytes (serde_json).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub seq: u64,
    pub timestamp_ms: i64,
    pub event: RuntimeEvent,
}

/// Filter for `RedbRuntimeEventJournal::inspect`.
pub struct JournalInspectFilter {
    pub route_id: Option<String>,
    pub limit: usize,
}

// ── Adapter ───────────────────────────────────────────────────────────────────

/// Redb-backed implementation of `RuntimeEventJournalPort`.
///
/// `Arc<Database>` allows cheap cloning — all clones share the same underlying
/// redb file handle. `redb::Database` is `Send + Sync`.
#[derive(Clone)]
pub struct RedbRuntimeEventJournal {
    db: Arc<Database>,
    options: RedbJournalOptions,
}

impl RedbRuntimeEventJournal {
    /// Open (or create) the redb database at `path`.
    ///
    /// Parent directories are created if they do not exist.
    /// Both tables are initialised on first open.
    /// Uses `tokio::task::spawn_blocking` because `Database::open` is blocking.
    pub async fn new(
        path: impl Into<PathBuf>,
        options: RedbJournalOptions,
    ) -> Result<Self, CamelError> {
        let path = path.into();
        let db = tokio::task::spawn_blocking(move || {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    CamelError::Io(format!(
                        "failed to create journal directory '{}': {e}",
                        parent.display()
                    ))
                })?;
            }
            let db = Database::create(&path).map_err(|e| {
                CamelError::Io(format!(
                    "failed to open journal at '{}': {e}",
                    path.display()
                ))
            })?;
            // Initialise tables so they exist before any reads.
            let tx = db
                .begin_write()
                .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
            tx.open_table(EVENTS_TABLE)
                .map_err(|e| CamelError::Io(format!("redb open events table: {e}")))?;
            tx.open_table(COMMAND_IDS_TABLE)
                .map_err(|e| CamelError::Io(format!("redb open command_ids table: {e}")))?;
            tx.commit()
                .map_err(|e| CamelError::Io(format!("redb commit init: {e}")))?;
            Ok::<_, CamelError>(db)
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))??;

        Ok(Self {
            db: Arc::new(db),
            options,
        })
    }

    /// Open an existing database at `path` and return entries (newest-first, up to `filter.limit`).
    ///
    /// Uses `Database::open` + `begin_read` — concurrent with a live writer on the same file.
    /// `inspect` is an offline utility: it does NOT require a live `RedbRuntimeEventJournal` instance.
    pub async fn inspect(
        path: impl Into<PathBuf>,
        filter: JournalInspectFilter,
    ) -> Result<Vec<JournalEntry>, CamelError> {
        let path = path.into();
        let limit = filter.limit;
        let route_id = filter.route_id;
        tokio::task::spawn_blocking(move || {
            if !path.exists() {
                return Err(CamelError::Io(format!(
                    "journal file not found: {}",
                    path.display()
                )));
            }
            let db = Database::open(&path)
                .map_err(|e| CamelError::Io(format!("invalid journal file: {e}")))?;
            let tx = db
                .begin_read()
                .map_err(|e| CamelError::Io(format!("redb begin_read: {e}")))?;
            let table = tx
                .open_table(EVENTS_TABLE)
                .map_err(|e| CamelError::Io(format!("redb open events: {e}")))?;

            // Collect in descending order (newest first).
            // Filter by route_id FIRST, then apply limit — ensures we return
            // `limit` matching entries, not `limit` total entries where most may
            // not match the filter.
            let mut entries: Vec<JournalEntry> = Vec::new();
            for result in table
                .iter()
                .map_err(|e| CamelError::Io(format!("redb iter: {e}")))?
                .rev()
            {
                let (_k, v) = result.map_err(|e| CamelError::Io(format!("redb read: {e}")))?;
                let entry: JournalEntry = serde_json::from_slice(v.value())
                    .map_err(|e| CamelError::Io(format!("journal deserialize: {e}")))?;
                if let Some(ref rid) = route_id
                    && entry.event.route_id() != rid.as_str()
                {
                    continue;
                }
                if entries.len() >= limit {
                    break;
                }
                entries.push(entry);
            }
            Ok(entries)
        })
        .await
        .map_err(|e| CamelError::Io(format!("spawn_blocking join: {e}")))?
    }

    // ── Internal helpers ──────────────────────────────────────────────────────

    fn redb_durability(&self) -> redb::Durability {
        match self.options.durability {
            JournalDurability::Immediate => redb::Durability::Immediate,
            JournalDurability::Eventual => redb::Durability::Eventual,
        }
    }

    /// Derive next sequence number from the last key in the events table.
    /// Must be called inside a write transaction with the table already open.
    fn next_seq(table: &redb::Table<u64, &[u8]>) -> Result<u64, CamelError> {
        match table
            .iter()
            .map_err(|e| CamelError::Io(format!("redb iter for seq: {e}")))?
            .next_back()
        {
            Some(Ok((k, _))) => Ok(k.value() + 1),
            Some(Err(e)) => Err(CamelError::Io(format!("redb seq read: {e}"))),
            None => Ok(0),
        }
    }

    /// Count rows in the events table (read transaction).
    fn event_count(&self) -> Result<u64, CamelError> {
        let tx = self
            .db
            .begin_read()
            .map_err(|e| CamelError::Io(format!("redb begin_read: {e}")))?;
        let table = tx
            .open_table(EVENTS_TABLE)
            .map_err(|e| CamelError::Io(format!("redb open events: {e}")))?;
        table
            .len()
            .map_err(|e| CamelError::Io(format!("redb len: {e}")))
    }

    /// Compact the events table: remove events for routes that have been fully removed.
    fn compact(&self) -> Result<(), CamelError> {
        let tx = self
            .db
            .begin_write()
            .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
        {
            let mut table = tx
                .open_table(EVENTS_TABLE)
                .map_err(|e| CamelError::Io(format!("redb open events: {e}")))?;

            // Pass 1: read all events in key order, find last RouteRemoved seq per route.
            let mut last_removed_seq: std::collections::HashMap<String, u64> =
                std::collections::HashMap::new();
            for result in table
                .iter()
                .map_err(|e| CamelError::Io(format!("redb iter: {e}")))?
            {
                let (k, v) = result.map_err(|e| CamelError::Io(format!("redb read: {e}")))?;
                let seq = k.value();
                let entry: JournalEntry = serde_json::from_slice(v.value())
                    .map_err(|e| CamelError::Io(format!("journal deserialize: {e}")))?;
                if matches!(entry.event, RuntimeEvent::RouteRemoved { .. }) {
                    last_removed_seq.insert(entry.event.route_id().to_string(), seq);
                }
            }

            if last_removed_seq.is_empty() {
                drop(table);
                tx.commit()
                    .map_err(|e| CamelError::Io(format!("redb commit compact: {e}")))?;
                return Ok(());
            }

            // Pass 2: collect seqs to delete.
            let mut to_delete: Vec<u64> = Vec::new();
            for result in table
                .iter()
                .map_err(|e| CamelError::Io(format!("redb iter pass2: {e}")))?
            {
                let (k, v) = result.map_err(|e| CamelError::Io(format!("redb read: {e}")))?;
                let seq = k.value();
                let entry: JournalEntry = serde_json::from_slice(v.value())
                    .map_err(|e| CamelError::Io(format!("journal deserialize: {e}")))?;
                let route_id = entry.event.route_id().to_string();
                if let Some(&cutoff) = last_removed_seq.get(&route_id)
                    && seq <= cutoff
                {
                    to_delete.push(seq);
                }
            }

            for seq in to_delete {
                table
                    .remove(&seq)
                    .map_err(|e| CamelError::Io(format!("redb remove seq {seq}: {e}")))?;
            }
        }
        tx.commit()
            .map_err(|e| CamelError::Io(format!("redb commit compact: {e}")))?;
        Ok(())
    }
}

// ── RuntimeEvent helper ───────────────────────────────────────────────────────

/// Extension to extract the `route_id` field from any `RuntimeEvent` variant.
trait RuntimeEventExt {
    fn route_id(&self) -> &str;
}

impl RuntimeEventExt for RuntimeEvent {
    fn route_id(&self) -> &str {
        match self {
            RuntimeEvent::RouteRegistered { route_id }
            | RuntimeEvent::RouteStartRequested { route_id }
            | RuntimeEvent::RouteStarted { route_id }
            | RuntimeEvent::RouteFailed { route_id, .. }
            | RuntimeEvent::RouteStopped { route_id }
            | RuntimeEvent::RouteSuspended { route_id }
            | RuntimeEvent::RouteResumed { route_id }
            | RuntimeEvent::RouteReloaded { route_id }
            | RuntimeEvent::RouteRemoved { route_id } => route_id,
        }
    }
}

// ── RuntimeEventJournalPort impl ──────────────────────────────────────────────

#[async_trait]
impl RuntimeEventJournalPort for RedbRuntimeEventJournal {
    async fn append_batch(&self, events: &[RuntimeEvent]) -> Result<(), DomainError> {
        if events.is_empty() {
            return Ok(());
        }
        let db = Arc::clone(&self.db);
        let durability = self.redb_durability();
        let events = events.to_vec();
        let now_ms = chrono::Utc::now().timestamp_millis();

        tokio::task::spawn_blocking(move || {
            // NOTE: `mut` is required — `set_durability` takes `&mut self` in redb v2.
            let mut tx = db
                .begin_write()
                .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
            tx.set_durability(durability);
            {
                let mut table = tx
                    .open_table(EVENTS_TABLE)
                    .map_err(|e| CamelError::Io(format!("redb open events: {e}")))?;
                let mut next_seq = Self::next_seq(&table)?;
                for event in events {
                    let entry = JournalEntry {
                        seq: next_seq,
                        timestamp_ms: now_ms,
                        event,
                    };
                    let bytes = serde_json::to_vec(&entry)
                        .map_err(|e| CamelError::Io(format!("journal serialize: {e}")))?;
                    table
                        .insert(&next_seq, bytes.as_slice())
                        .map_err(|e| CamelError::Io(format!("redb insert: {e}")))?;
                    next_seq += 1;
                }
            }
            tx.commit()
                .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
            Ok::<_, CamelError>(())
        })
        .await
        .map_err(|e| DomainError::InvalidState(format!("spawn_blocking join: {e}")))?
        .map_err(|e| DomainError::InvalidState(e.to_string()))?;

        // Trigger compaction if threshold exceeded. Non-fatal if it fails.
        // Both event_count() and compact() do blocking redb I/O — run in spawn_blocking.
        let journal_clone = self.clone();
        let threshold = self.options.compaction_threshold_events;
        tokio::task::spawn_blocking(move || match journal_clone.event_count() {
            Ok(count) if count >= threshold => {
                if let Err(e) = journal_clone.compact() {
                    tracing::warn!("journal compaction failed (non-fatal): {e}");
                }
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!("journal event count check failed (non-fatal): {e}");
            }
        })
        .await
        .ok(); // Non-fatal: if spawn_blocking panics, we ignore it

        Ok(())
    }

    async fn load_all(&self) -> Result<Vec<RuntimeEvent>, DomainError> {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || {
            let tx = db
                .begin_read()
                .map_err(|e| CamelError::Io(format!("redb begin_read: {e}")))?;
            let table = tx
                .open_table(EVENTS_TABLE)
                .map_err(|e| CamelError::Io(format!("redb open events: {e}")))?;
            let mut events = Vec::new();
            for result in table
                .iter()
                .map_err(|e| CamelError::Io(format!("redb iter: {e}")))?
            {
                let (_k, v) = result.map_err(|e| CamelError::Io(format!("redb read: {e}")))?;
                let entry: JournalEntry = serde_json::from_slice(v.value())
                    .map_err(|e| CamelError::Io(format!("journal deserialize: {e}")))?;
                events.push(entry.event);
            }
            Ok(events)
        })
        .await
        .map_err(|e| DomainError::InvalidState(format!("spawn_blocking join: {e}")))?
        .map_err(|e: CamelError| DomainError::InvalidState(e.to_string()))
    }

    async fn append_command_id(&self, command_id: &str) -> Result<(), DomainError> {
        let db = Arc::clone(&self.db);
        let durability = self.redb_durability();
        let id = command_id.to_string();
        tokio::task::spawn_blocking(move || {
            // NOTE: `mut` required — `set_durability` takes `&mut self` in redb v2.
            let mut tx = db
                .begin_write()
                .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
            tx.set_durability(durability);
            {
                let mut table = tx
                    .open_table(COMMAND_IDS_TABLE)
                    .map_err(|e| CamelError::Io(format!("redb open command_ids: {e}")))?;
                table
                    .insert(id.as_str(), ())
                    .map_err(|e| CamelError::Io(format!("redb insert command_id: {e}")))?;
            }
            tx.commit()
                .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
            Ok::<_, CamelError>(())
        })
        .await
        .map_err(|e| DomainError::InvalidState(format!("spawn_blocking join: {e}")))?
        .map_err(|e| DomainError::InvalidState(e.to_string()))
    }

    async fn remove_command_id(&self, command_id: &str) -> Result<(), DomainError> {
        let db = Arc::clone(&self.db);
        let durability = self.redb_durability();
        let id = command_id.to_string();
        tokio::task::spawn_blocking(move || {
            // NOTE: `mut` required — `set_durability` takes `&mut self` in redb v2.
            let mut tx = db
                .begin_write()
                .map_err(|e| CamelError::Io(format!("redb begin_write: {e}")))?;
            tx.set_durability(durability);
            {
                let mut table = tx
                    .open_table(COMMAND_IDS_TABLE)
                    .map_err(|e| CamelError::Io(format!("redb open command_ids: {e}")))?;
                table
                    .remove(id.as_str())
                    .map_err(|e| CamelError::Io(format!("redb remove command_id: {e}")))?;
            }
            tx.commit()
                .map_err(|e| CamelError::Io(format!("redb commit: {e}")))?;
            Ok::<_, CamelError>(())
        })
        .await
        .map_err(|e| DomainError::InvalidState(format!("spawn_blocking join: {e}")))?
        .map_err(|e| DomainError::InvalidState(e.to_string()))
    }

    async fn load_command_ids(&self) -> Result<Vec<String>, DomainError> {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || {
            let tx = db
                .begin_read()
                .map_err(|e| CamelError::Io(format!("redb begin_read: {e}")))?;
            let table = tx
                .open_table(COMMAND_IDS_TABLE)
                .map_err(|e| CamelError::Io(format!("redb open command_ids: {e}")))?;
            let mut ids = Vec::new();
            for result in table
                .iter()
                .map_err(|e| CamelError::Io(format!("redb iter: {e}")))?
            {
                let (k, _) = result.map_err(|e| CamelError::Io(format!("redb read: {e}")))?;
                ids.push(k.value().to_string());
            }
            Ok(ids)
        })
        .await
        .map_err(|e| DomainError::InvalidState(format!("spawn_blocking join: {e}")))?
        .map_err(|e: CamelError| DomainError::InvalidState(e.to_string()))
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    async fn new_journal(dir: &tempfile::TempDir) -> RedbRuntimeEventJournal {
        RedbRuntimeEventJournal::new(dir.path().join("test.db"), RedbJournalOptions::default())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn redb_journal_roundtrip() {
        let dir = tempdir().unwrap();
        let journal = new_journal(&dir).await;

        let events = vec![
            RuntimeEvent::RouteRegistered {
                route_id: "r1".to_string(),
            },
            RuntimeEvent::RouteStarted {
                route_id: "r1".to_string(),
            },
        ];
        journal.append_batch(&events).await.unwrap();

        let loaded = journal.load_all().await.unwrap();
        assert_eq!(loaded, events);
    }

    #[tokio::test]
    async fn redb_journal_command_id_lifecycle() {
        let dir = tempdir().unwrap();
        let journal = new_journal(&dir).await;

        journal.append_command_id("c1").await.unwrap();
        journal.append_command_id("c2").await.unwrap();
        journal.remove_command_id("c1").await.unwrap();

        let ids = journal.load_command_ids().await.unwrap();
        assert_eq!(ids, vec!["c2".to_string()]);
    }

    #[tokio::test]
    async fn redb_journal_compaction_removes_completed_routes() {
        let dir = tempdir().unwrap();
        // Threshold of 1 triggers compaction on every append.
        let journal = RedbRuntimeEventJournal::new(
            dir.path().join("compact.db"),
            RedbJournalOptions {
                durability: JournalDurability::Eventual,
                compaction_threshold_events: 1,
            },
        )
        .await
        .unwrap();

        // Removed route — full lifecycle.
        journal
            .append_batch(&[RuntimeEvent::RouteRegistered {
                route_id: "old".to_string(),
            }])
            .await
            .unwrap();
        journal
            .append_batch(&[RuntimeEvent::RouteRemoved {
                route_id: "old".to_string(),
            }])
            .await
            .unwrap();

        // Active route — no RouteRemoved.
        journal
            .append_batch(&[RuntimeEvent::RouteRegistered {
                route_id: "live".to_string(),
            }])
            .await
            .unwrap();

        let loaded = journal.load_all().await.unwrap();
        assert!(
            !loaded.iter().any(
                |e| matches!(e, RuntimeEvent::RouteRegistered { route_id } if route_id == "old")
            ),
            "old route events must be compacted"
        );
        assert!(
            loaded.iter().any(
                |e| matches!(e, RuntimeEvent::RouteRegistered { route_id } if route_id == "live")
            ),
            "live route events must survive compaction"
        );
    }

    #[tokio::test]
    async fn redb_journal_compaction_preserves_reregistered_route() {
        let dir = tempdir().unwrap();
        let journal = RedbRuntimeEventJournal::new(
            dir.path().join("rereg.db"),
            RedbJournalOptions {
                durability: JournalDurability::Eventual,
                compaction_threshold_events: 1,
            },
        )
        .await
        .unwrap();

        journal
            .append_batch(&[RuntimeEvent::RouteRegistered {
                route_id: "rereg".to_string(),
            }])
            .await
            .unwrap();
        journal
            .append_batch(&[RuntimeEvent::RouteRemoved {
                route_id: "rereg".to_string(),
            }])
            .await
            .unwrap();
        journal
            .append_batch(&[RuntimeEvent::RouteRegistered {
                route_id: "rereg".to_string(),
            }])
            .await
            .unwrap();

        let loaded = journal.load_all().await.unwrap();
        let rereg_count = loaded
            .iter()
            .filter(
                |e| matches!(e, RuntimeEvent::RouteRegistered { route_id } if route_id == "rereg"),
            )
            .count();
        assert_eq!(
            rereg_count, 1,
            "re-registered route must have exactly one event after compaction"
        );
    }

    #[tokio::test]
    async fn redb_journal_durability_eventual() {
        let dir = tempdir().unwrap();
        let journal = RedbRuntimeEventJournal::new(
            dir.path().join("eventual.db"),
            RedbJournalOptions {
                durability: JournalDurability::Eventual,
                compaction_threshold_events: 10_000,
            },
        )
        .await
        .unwrap();

        journal
            .append_batch(&[RuntimeEvent::RouteRegistered {
                route_id: "ev".to_string(),
            }])
            .await
            .unwrap();
        let loaded = journal.load_all().await.unwrap();
        assert_eq!(loaded.len(), 1);
    }

    #[tokio::test]
    async fn redb_journal_clone_shares_db() {
        let dir = tempdir().unwrap();
        let j1 = new_journal(&dir).await;
        let j2 = j1.clone();

        j1.append_batch(&[RuntimeEvent::RouteRegistered {
            route_id: "shared".to_string(),
        }])
        .await
        .unwrap();

        // j2 must see j1's write since they share the same Arc<Database>.
        let loaded = j2.load_all().await.unwrap();
        assert_eq!(loaded.len(), 1);
    }
}
