//! rc-9awnc conformance fixtures: absolute deadline ordering across SQL
//! manager admission.
//!
//! A transacted block runs with two deadlines: the block timeout (the frozen
//! spec "Block timeout" timer the segment arms at block entry) and the
//! manager admission timeout (the pool acquire the session races inside
//! `open`). Absolute deadline ordering says the earlier deadline wins the
//! reported primary, including the exact tie and the both-ready case — the
//! real block timer never loses to a ready `PoolExhausted`.
//!
//! The fixtures drive the PRODUCTION manager
//! ([`SqlTransactionManager`]) through the real
//! [`TransactedSegment`] under paused `tokio` virtual time — no mock
//! manager. The pool is a single-connection in-memory SQLite whose only
//! connection is held by the test, so checkout waits until a deadline
//! fires; the frozen names are asserted on the segment's
//! [`PipelineOutcome::Failed`] primary:
//!
//! - [`earlier_admission_deadline_reports_pool_exhausted`] — acquire
//!   strictly first → typed `PoolExhausted`.
//! - [`earlier_block_deadline_reports_timeout`] — block strictly first →
//!   typed `Timeout` measured from block entry.
//! - [`exact_tie_reports_block_timeout`] — both deadlines at the same
//!   instant → `Timeout` (the tie belongs to the block).
//! - [`both_ready_after_block_deadline_reports_block_timeout`] — both
//!   deadlines expire in one advance with the block strictly first (30 ms
//!   against 40 ms) → `Timeout`; a ready `PoolExhausted` after the spent
//!   block deadline is re-adjudicated (rc-9awnc), which also pins the
//!   strict first-wins half of the guard.
//! - [`pending_open_does_not_outlive_block_deadline`] — an open still
//!   waiting at the block deadline resolves the block at the deadline and
//!   leaves the held connection usable.
//!
//! The wrong-winner fixtures (tie, both-ready) fail against a segment that
//! accepts a ready `PoolExhausted` unconditionally — they guard the
//! re-adjudication at `crates/camel-core/src/transacted_segment.rs`
//! (`block_abs <= acquire_abs` → `Timeout`; mutating `<=` to `==` breaks
//! [`both_ready_after_block_deadline_reports_block_timeout`]).
//!
//! Home: this suite rides `camel-test` (the ADR-0055 publish-order leaf
//! sink) behind the narrow `integration-sql` feature. `camel-component-sql`
//! must not dev-depend on `camel-core` — `camel-core` already dev-depends on
//! `camel-component-sql`, so that edge would close a publish cycle.

#![cfg(feature = "integration-sql")]

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use camel_api::Exchange;
use camel_api::Message;
use camel_api::OutcomePipeline;
use camel_api::OutcomeSegment;
use camel_api::PipelineOutcome;
use camel_api::body::Body;
use camel_api::datasource::DatasourceCatalog;
use camel_api::datasource::DatasourceConfig;
use camel_api::datasource::DatasourceHandle;
use camel_api::datasource::GetPoolFuture;
use camel_api::error::CamelError;
use camel_api::transaction::TransactionError;
use camel_api::transaction::TransactionManager;
use camel_component_sql::SqlTransactionManager;
use camel_core::transacted_segment::TransactedSegment;
use sqlx::AnyPool;
use sqlx::any::AnyPoolOptions;

/// Datasource every typed error names.
const DS: &str = "pg";

/// Block deadline for every fixture: 30 ms after block entry.
const BLOCK: Duration = Duration::from_millis(30);

/// Acquire deadline that never competes with the block deadline (60 s).
const ACQUIRE_LATE: Duration = Duration::from_secs(60);

/// Acquire deadline strictly inside the block deadline (10 ms).
const ACQUIRE_EARLY: Duration = Duration::from_millis(10);

/// The block body: never reached in these fixtures (every block ends at an
/// admission deadline); returns the exchange unchanged.
#[derive(Clone)]
struct NoopBody;

impl OutcomePipeline for NoopBody {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move { PipelineOutcome::Completed(exchange) })
    }
}

/// Catalog serving one pre-built pool for every datasource name, mirroring
/// the boot catalog the bundle registers managers against.
struct FixedCatalog {
    pool: AnyPool,
    config: DatasourceConfig,
}

impl DatasourceCatalog for FixedCatalog {
    fn get_config(&self, _name: &str) -> Option<DatasourceConfig> {
        Some(self.config.clone())
    }

    fn get_pool<'a>(&'a self, name: &'a str) -> GetPoolFuture<'a> {
        let pool = self.pool.clone();
        let name = name.to_string();
        Box::pin(async move {
            let inner: Arc<dyn std::any::Any + Send + Sync> = Arc::new(pool);
            Ok(DatasourceHandle::new(name, "sqlx".to_string(), inner))
        })
    }

    fn register_factory(
        &self,
        _kind: &str,
        _factory: Arc<dyn camel_api::datasource::PoolFactory>,
    ) -> Result<(), CamelError> {
        Ok(())
    }
}

fn datasource_config() -> DatasourceConfig {
    DatasourceConfig {
        db_url: "postgres://camel:camel@127.0.0.1:5432/camel".to_string(),
        provider: Some("sqlx".to_string()),
        max_connections: Some(1),
        min_connections: Some(0),
        idle_timeout_secs: None,
        max_lifetime_secs: None,
        ssl_mode: None,
        ssl_root_cert: None,
        ssl_cert: None,
        ssl_key: None,
        extra: std::collections::HashMap::new(),
    }
}

/// Single-connection in-memory pool whose only connection is held by the
/// returned guard, so any block checkout waits for a deadline. Connect and
/// hold run on the REAL clock (sqlx's own 30 s acquire timeout is a real
/// timer here); every test then pauses the clock and drives the block with
/// explicit `tokio::time::advance`, so deadline wakes are exact.
async fn held_pool() -> (AnyPool, sqlx::pool::PoolConnection<sqlx::Any>) {
    sqlx::any::install_default_drivers();
    let pool = AnyPoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        // allow-test-wait: test-local in-memory sqlite connect; immediate, bounded by construction
        .await
        .expect("in-memory sqlite pool");
    let held = pool
        .acquire()
        // allow-test-wait: test-local sqlite pool's only connection is free; acquire resolves immediately
        .await
        .expect("held connection");
    (pool, held)
}

/// One block execution through the production manager and the real segment,
/// spawned on the paused clock. The helper yields once after spawning, so
/// the segment records its block entry on the task's first poll — BEFORE
/// the caller advances time. The caller then drives the deadline wake with
/// a single explicit `tokio::time::advance` past the deadlines under test;
/// `block` is the block deadline, `acquire` the manager's admission bound.
async fn spawn_block(
    pool: &AnyPool,
    block: Duration,
    acquire: Duration,
) -> tokio::task::JoinHandle<PipelineOutcome> {
    let catalog: Arc<dyn DatasourceCatalog> = Arc::new(FixedCatalog {
        pool: pool.clone(),
        config: datasource_config(),
    });
    let manager: Arc<dyn TransactionManager> =
        Arc::new(SqlTransactionManager::new(DS, catalog).with_acquire_deadline(acquire));
    let mut segment = TransactedSegment::new(DS, manager, OutcomeSegment::new(Box::new(NoopBody)))
        .with_block_timeout(block);
    let handle =
        tokio::spawn(async move { segment.run(Exchange::new(Message::new(Body::Empty))).await });
    // First poll at the paused instant: block entry must precede the advance
    // or every deadline would shift by the advance and auto-advance would
    // pick the wake.
    tokio::task::yield_now().await;
    handle
}

/// The typed transaction error behind a failed block outcome.
fn tx_error_of(outcome: PipelineOutcome) -> TransactionError {
    match outcome {
        PipelineOutcome::Failed(CamelError::Transaction(e)) => e,
        PipelineOutcome::Failed(other) => {
            panic!("expected a transaction error, got: {other}")
        }
        PipelineOutcome::Completed(_) => {
            panic!("block completed; expected a deadline adjudication failure")
        }
        PipelineOutcome::Stopped(_) => {
            panic!("block stopped; expected a deadline adjudication failure")
        }
    }
}

/// Admission strictly first → typed `PoolExhausted` naming the datasource,
/// the pool size, and the waited milliseconds (frozen "Pool exhaustion is
/// bounded and named").
#[tokio::test]
async fn earlier_admission_deadline_reports_pool_exhausted() {
    let (pool, _held) = held_pool().await;
    tokio::time::pause();

    let run = spawn_block(&pool, BLOCK, ACQUIRE_EARLY).await;
    tokio::time::advance(Duration::from_millis(11)).await;
    let outcome = run.await.expect("block task");
    match tx_error_of(outcome) {
        TransactionError::PoolExhausted {
            datasource,
            max_connections,
            waited_ms,
        } => {
            assert_eq!(datasource, DS);
            assert_eq!(max_connections, 1);
            assert!(
                (10..=11).contains(&waited_ms),
                "waited_ms must measure the 10 ms acquire deadline, got {waited_ms}"
            );
        }
        other => panic!("expected typed PoolExhausted, got {other:?}"),
    }
}

/// Block strictly first → typed `Timeout` measured from block entry (frozen
/// "Block timeout"), never `PoolExhausted`.
#[tokio::test]
async fn earlier_block_deadline_reports_timeout() {
    let (pool, _held) = held_pool().await;
    tokio::time::pause();

    let run = spawn_block(&pool, BLOCK, ACQUIRE_LATE).await;
    tokio::time::advance(Duration::from_millis(31)).await;
    let outcome = run.await.expect("block task");
    match tx_error_of(outcome) {
        TransactionError::Timeout {
            datasource,
            elapsed_ms,
        } => {
            assert_eq!(datasource, DS);
            assert!(
                (30..=31).contains(&elapsed_ms),
                "elapsed_ms must measure the 30 ms block deadline, got {elapsed_ms}"
            );
        }
        other => panic!("expected typed Timeout, got {other:?}"),
    }
}

/// Exact tie → the block timer wins: the ready `PoolExhausted` that resolves
/// at the shared deadline is re-adjudicated to `Timeout` by absolute
/// instants (rc-9awnc).
#[tokio::test]
async fn exact_tie_reports_block_timeout() {
    let (pool, _held) = held_pool().await;
    tokio::time::pause();

    let run = spawn_block(&pool, BLOCK, BLOCK).await;
    tokio::time::advance(Duration::from_millis(31)).await;
    let outcome = run.await.expect("block task");
    match tx_error_of(outcome) {
        TransactionError::Timeout {
            datasource,
            elapsed_ms,
        } => {
            assert_eq!(datasource, DS);
            assert!(
                (30..=31).contains(&elapsed_ms),
                "elapsed_ms must measure the 30 ms block deadline, got {elapsed_ms}"
            );
        }
        other => panic!("expected typed Timeout on the exact tie, got {other:?}"),
    }
}

/// Both-ready after a block-first deadline → `Timeout`: one advance expires
/// the 30 ms block deadline and the 40 ms acquire deadline together, the
/// block deadline is strictly first, and the ready `PoolExhausted` the open
/// resolves with is re-adjudicated to `Timeout` from entry (rc-9awnc).
/// This pins the strict first-wins half of the guard: mutating
/// `block_abs <= acquire_abs` to `==` makes this fixture fail with
/// `PoolExhausted`.
#[tokio::test]
async fn both_ready_after_block_deadline_reports_block_timeout() {
    let (pool, _held) = held_pool().await;
    tokio::time::pause();

    let block = Duration::from_millis(30);
    let acquire = Duration::from_millis(40);
    let run = spawn_block(&pool, block, acquire).await;
    // One advance past both deadlines: both timers expire in this single
    // wake, the biased open arm resolves first, and the block timer is
    // already spent when it does.
    tokio::time::advance(Duration::from_millis(50)).await;
    let outcome = run.await.expect("block task");
    match tx_error_of(outcome) {
        TransactionError::Timeout {
            datasource,
            elapsed_ms,
        } => {
            assert_eq!(datasource, DS);
            assert!(
                (50..=51).contains(&elapsed_ms),
                "elapsed_ms must measure the single 50 ms advance, got {elapsed_ms}"
            );
        }
        other => panic!(
            "expected typed Timeout for a ready PoolExhausted after the block deadline, got {other:?}"
        ),
    }
}

/// A pending open never outlives the block deadline: the block resolves at
/// the deadline (Timeout measured from entry, never the 60 s acquire bound),
/// and the held connection stays usable afterwards. The R0 discard-detach
/// path does not run here — a pending open never checked a connection out.
#[tokio::test]
async fn pending_open_does_not_outlive_block_deadline() {
    let (pool, mut held) = held_pool().await;
    tokio::time::pause();

    let run = spawn_block(&pool, BLOCK, ACQUIRE_LATE).await;
    tokio::time::advance(Duration::from_millis(31)).await;
    let outcome = run.await.expect("block task");
    match tx_error_of(outcome) {
        TransactionError::Timeout {
            datasource,
            elapsed_ms,
        } => {
            assert_eq!(datasource, DS);
            assert!(
                (30..=31).contains(&elapsed_ms),
                "the block must resolve at the 30 ms deadline, got {elapsed_ms} ms"
            );
        }
        other => panic!("expected typed Timeout for the pending open, got {other:?}"),
    }

    // The dropped open left no poisoned backend: the pool's live connection
    // still serves statements. (Re-acquiring from the same pool is not
    // assertable under the paused clock — sqlx returns a PoolConnection on
    // drop through real-time churn a paused clock does not drive — so the
    // usability probe goes through the connection the test itself holds.)
    let row: (i64,) = sqlx::query_as("SELECT 1")
        .fetch_one(&mut *held)
        // allow-test-wait: real-time local sqlite SELECT on the held connection; immediate
        .await
        .expect("the held connection must stay usable after a deadline-dropped open");
    assert_eq!(row.0, 1);
}
