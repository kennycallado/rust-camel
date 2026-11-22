//! PG runtime fixtures for the transacted-block probes P1–P6
//! (`sqltrans-exec` step 8).
//!
//! Probes were observed against PostgreSQL 16.10-alpine with sqlx 0.9.0
//! (`/home/shared/tmp/sqltrans-exec-verify/probe-digest.md`). These fixtures
//! turn them into the frozen names from
//! `openspec/changes/sqltrans/specs/sql-transactions/spec.md`:
//!
//! - [`transacted_commit_deferred_fk`] — P1: a `DEFERRABLE INITIALLY
//!   DEFERRED` FK violation surfaces at COMMIT as `23503` and maps to the
//!   typed `CommitFailed`; no row persists; the pool stays usable.
//!   [`transacted_commit_rejected`] is the spec's alias for the same flow.
//! - [`transacted_backend_kill_commit_uncertain`] — P2:
//!   `pg_terminate_backend` from a deferred constraint trigger kills the
//!   backend inside COMMIT (`FATAL 57P01`) → `CommitUncertain`, no rollback
//!   claim, and the pool serves a NEW backend afterwards (the spec's
//!   Docker-suite backend-termination fixture).
//! - [`transacted_cancel_before_commit`] — P3a: cancellation before the
//!   driver handoff → bounded (`< 5 s` grace) rollback, no write persists.
//!   The cancellation-TIMING scenarios
//!   (`transacted_cancel_after_commit_ack` / `transacted_cancel_after_commit_lost`)
//!   are segment-tier fixtures in `camel-core::transacted_segment` —
//!   CANCEL_TOKEN is a camel-core crate-private task-local, so the timing
//!   cannot be installed from this suite.
//! - [`transacted_commit_ack_stray_rollback_noop`] — P3b: after an
//!   acknowledged COMMIT a stray `ROLLBACK` is a server-side no-op and the
//!   committed row survives.
//! - [`transacted_prior_borrower_discard`] — P4: `max_connections=1`
//!   abandoned transaction; the admission hygiene ROLLBACK discards it, the
//!   same backend PID proves no replacement can satisfy the assertion, and
//!   the naive-`BEGIN` poisoning hazard is pinned as the regression that
//!   justifies mandatory hygiene.
//! - [`transacted_pool_exhausted_typed`] — P5: a held `max_connections=1`
//!   slot + the acquire bound yields the typed `PoolExhausted` naming
//!   datasource, `max_connections`, and `waited_ms`.
//! - [`transacted_busy_slot_deterministic`] — P6: a statement holding the
//!   session's single execution slot makes a concurrent admission return the
//!   typed `Busy` without awaiting.
//!
//! Path realism: step 9 registered `SqlTransactionManager` per catalog
//! datasource, so the fixtures
//! drive the real `SqlTxSession` through a test-only [`TransactionManager`]
//! and `TransactedSegment`; the block body joins through the real
//! `BLOCK_SESSION` task-local and executes on the session's connection (the
//! lent-connection stand-in for the joined `sql:` step). Session-level
//! `SQLTRANS_PG_URL` (§ below) keeps the CI-owned testcontainers path for the
//! rest of the suite untouched.
//!
//! Database selection: when `SQLTRANS_PG_URL` is set the fixtures use that
//! URL (locally a `--network host` PostgreSQL container, because this host's
//! mapped-container-port data leg is dead); otherwise they start the shared
//! testcontainers PostgreSQL exactly like `sql_test.rs`. Each fixture's body
//! is identical on both paths.

#![cfg(feature = "integration-tests")]

mod support;

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use camel_api::Exchange;
use camel_api::OutcomePipeline;
use camel_api::OutcomeSegment;
use camel_api::PipelineOutcome;
use camel_api::body::Body;
use camel_api::transaction::OpenSessionFuture;
use camel_api::transaction::TransactionError;
use camel_api::transaction::TransactionManager;
use camel_api::transaction::TransactionOutcome;
use camel_api::transaction::TransactionSession;
use camel_api::transaction::join::current_block_session;
use camel_component_api::Message;
use camel_component_sql::SqlTxSession;
use camel_core::transacted_segment::TransactedSegment;
use sqlx::AnyPool;

use support::wait::wait_until;

/// Datasource name every fixture names in its typed errors.
const DS: &str = "pg";

/// Block entry + 30 s: the segment's own bound, mirrored by the test manager.
const BLOCK_DEADLINE: Duration = Duration::from_secs(30);

/// Acquisition bound used by every fixture except P5 (which owns its own).
const ACQUIRE: Duration = Duration::from_secs(5);

/// Exchange handed to the segment; the fixtures assert transaction outcomes,
/// not body content.
fn exchange() -> Exchange {
    Exchange::new(Message::new(Body::Empty))
}

// ── Database selection ────────────────────────────────────────────────────

/// `SQLTRANS_PG_URL` when set, else the shared testcontainers PostgreSQL.
///
/// The override exists for hosts whose mapped-container-port data leg is dead
/// (see the module docs); the default path is the CI-owned one.
async fn pg_url() -> &'static str {
    static OVERRIDE: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();
    if let Some(url) = OVERRIDE.get() {
        return url;
    }
    if let Ok(url) = std::env::var("SQLTRANS_PG_URL") {
        return OVERRIDE.get_or_init(|| Box::leak(url.into_boxed_str()));
    }
    support::postgres::shared_postgres().await
}

fn install_drivers() {
    support::install_crypto_provider();
    sqlx::any::install_default_drivers();
}

async fn new_pool(url: &str, max_connections: u32, acquire_timeout: Duration) -> AnyPool {
    install_drivers();
    tokio::time::timeout(
        Duration::from_secs(30),
        sqlx::any::AnyPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(acquire_timeout)
            .connect(url),
    )
    .await
    .expect("sql pool connect timed out after 30s")
    .expect("failed to connect to PostgreSQL")
}

async fn run_sql(pool: &AnyPool, sql: &str) {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(pool)
        .await
        .unwrap_or_else(|error| panic!("statement failed: {sql}: {error}"));
}

async fn run_ddl(pool: &AnyPool, statements: &[&str]) {
    for statement in statements {
        run_sql(pool, statement).await;
    }
}

async fn scalar_i64(pool: &AnyPool, sql: &str) -> i64 {
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(pool)
        .await
        .unwrap_or_else(|error| panic!("scalar query failed: {sql}: {error}"))
}

/// Bounded pool acquisition: a fixture never parks on the pool without a
/// deadline.
async fn acquire_conn(pool: &AnyPool) -> sqlx::pool::PoolConnection<sqlx::Any> {
    tokio::time::timeout(Duration::from_secs(5), pool.acquire())
        .await
        .expect("pool acquisition exceeded the 5 s fixture bound")
        .expect("pool connection")
}

async fn table_count(pool: &AnyPool, table: &str) -> i64 {
    scalar_i64(pool, &format!("SELECT COUNT(*) FROM {table}")).await
}

// ── Test-only manager + body ──────────────────────────────────────────────

/// `TransactionManager` over a real pool: the step-9 `SqlTransactionManager`
/// stand-in. `open_session` is exactly the production sequence (checkout +
/// hygiene through [`SqlTxSession::open_on_pool`]).
struct PgManager {
    pool: AnyPool,
    acquire_deadline: Duration,
}

impl TransactionManager for PgManager {
    fn datasource(&self) -> &str {
        DS
    }

    fn open_session(&self) -> OpenSessionFuture<'_> {
        Box::pin(async move {
            let block_abs = tokio::time::Instant::now() + BLOCK_DEADLINE;
            SqlTxSession::open_on_pool(DS, &self.pool, block_abs, self.acquire_deadline)
                .await
                .map(|session| Arc::new(session) as Arc<dyn TransactionSession>)
        })
    }

    fn acquire_deadline(&self) -> Duration {
        self.acquire_deadline
    }
}

fn segment(pool: &AnyPool, acquire_deadline: Duration, body: SessionBody) -> TransactedSegment {
    let manager: Arc<dyn TransactionManager> = Arc::new(PgManager {
        pool: pool.clone(),
        acquire_deadline,
    });
    TransactedSegment::new(DS, manager, OutcomeSegment::new(Box::new(body)))
}

/// The block body: joins the segment's `BLOCK_SESSION` (the real join path a
/// compiled `sql:` step will use), begins lazily on the first statement, and
/// runs its static statements on the session's own connection.
///
/// The optional PID observer runs while the statement's transaction is still
/// open (`idle in transaction`), so P2 can name the backend the deferred
/// trigger kills even though the kill loses the COMMIT acknowledgment.
#[derive(Clone)]
struct SessionBody {
    statements: Vec<&'static str>,
    pid_observer: Option<(AnyPool, Arc<StdMutex<Option<i32>>>)>,
}

impl SessionBody {
    fn statements(statements: Vec<&'static str>) -> Self {
        Self {
            statements,
            pid_observer: None,
        }
    }

    fn with_pid_observer(
        statements: Vec<&'static str>,
        pool: &AnyPool,
        cell: Arc<StdMutex<Option<i32>>>,
    ) -> Self {
        Self {
            statements,
            pid_observer: Some((pool.clone(), cell)),
        }
    }
}

impl OutcomePipeline for SessionBody {
    fn clone_box(&self) -> Box<dyn OutcomePipeline> {
        Box::new(self.clone())
    }

    fn run<'a>(
        &'a mut self,
        exchange: Exchange,
    ) -> Pin<Box<dyn Future<Output = PipelineOutcome> + Send + 'a>> {
        Box::pin(async move {
            let block = current_block_session()
                .expect("a transacted body always runs inside a BLOCK_SESSION scope");
            let session = block.session();
            let any: Arc<dyn Any + Send + Sync> = session;
            let Ok(sql_session) = any.downcast::<SqlTxSession>() else {
                panic!("BLOCK_SESSION did not carry a SqlTxSession");
            };

            sql_session
                .begin()
                .await
                .expect("lazy BEGIN on the block session");
            for statement in &self.statements {
                sql_session
                    .exec_statement_for_test(statement)
                    .await
                    .unwrap_or_else(|error| panic!("block body statement failed: {error}"));
            }

            if let Some((pool, cell)) = &self.pid_observer {
                let pid: i32 = sqlx::query_scalar(
                    "SELECT pid FROM pg_stat_activity \
                     WHERE state = 'idle in transaction' \
                       AND query LIKE '%sqltrans_p2_rows%'",
                )
                .fetch_one(pool)
                .await
                .expect("the observer cannot see the block's idle-in-transaction backend");
                *cell
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(pid);
            }

            PipelineOutcome::Completed(exchange)
        })
    }
}

// ── P1 ────────────────────────────────────────────────────────────────────

/// Serializes the two 23503 fixtures below: they share one table pair because
/// the statements sent on the session connection are static text, so the
/// suite's parallel default must not race their DDL.
static DEFERRED_FK_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn hold_deferred_fk_lock() -> tokio::sync::MutexGuard<'static, ()> {
    tokio::time::timeout(Duration::from_secs(60), DEFERRED_FK_LOCK.lock())
        .await
        .expect("the 23503 fixture lock must free within the suite's bound")
}

/// P1 — frozen fixture `transacted_commit_deferred_fk`: a deferred FK
/// violation is detected only at COMMIT and maps to the typed
/// `CommitFailed`; no row persists and the pool stays usable.
#[tokio::test(flavor = "multi_thread")]
async fn transacted_commit_deferred_fk() {
    assert_deferred_fk_commit_rejected().await;
}

/// Frozen fixture `transacted_commit_rejected` (spec cancellation cutoff): the
/// spec's two scenarios point at the same 23503 flow — a COMMIT returning a
/// database error fails typed and cleans up, and the deferred-FK shape is the
/// PostgreSQL form of it. Thin alias over the `transacted_commit_deferred_fk`
/// body.
#[tokio::test(flavor = "multi_thread")]
async fn transacted_commit_rejected() {
    assert_deferred_fk_commit_rejected().await;
}

async fn assert_deferred_fk_commit_rejected() {
    let _serial = hold_deferred_fk_lock().await;
    let url = pg_url().await;
    let pool = new_pool(url, 4, ACQUIRE).await;
    run_ddl(
        &pool,
        &[
            "DROP TABLE IF EXISTS sqltrans_p1_child CASCADE",
            "DROP TABLE IF EXISTS sqltrans_p1_parent CASCADE",
            "CREATE TABLE sqltrans_p1_parent (id BIGINT PRIMARY KEY)",
            "CREATE TABLE sqltrans_p1_child (\
                 id SERIAL PRIMARY KEY, \
                 pid BIGINT REFERENCES sqltrans_p1_parent(id) \
                     DEFERRABLE INITIALLY DEFERRED)",
        ],
    )
    .await;

    let mut segment = segment(
        &pool,
        ACQUIRE,
        SessionBody::statements(vec!["INSERT INTO sqltrans_p1_child (pid) VALUES (999)"]),
    );
    let outcome = segment.run(exchange()).await;

    // The INSERT was accepted inside the transaction (the body completed it);
    // COMMIT is where PostgreSQL answers 23503.
    match outcome {
        PipelineOutcome::Failed(camel_api::CamelError::Transaction(
            TransactionError::CommitFailed { datasource, reason },
        )) => {
            assert_eq!(datasource, DS, "CommitFailed names the datasource");
            assert!(
                reason.contains("23503"),
                "CommitFailed reason carries SQLSTATE 23503: {reason}"
            );
            assert!(
                reason.contains("sqltrans_p1_child_pid_fkey"),
                "CommitFailed reason names the constraint: {reason}"
            );
        }
        other => panic!("expected typed CommitFailed, got {other:?}"),
    }

    assert_eq!(
        table_count(&pool, "sqltrans_p1_child").await,
        0,
        "the rejected COMMIT must persist no row"
    );
    assert_eq!(
        scalar_i64(&pool, "SELECT 1").await,
        1,
        "the pool must stay usable after the rejected COMMIT"
    );
}

// ── P2 ────────────────────────────────────────────────────────────────────

/// P2 — the spec's backend-termination `CommitUncertain` fixture: a backend
/// killed inside COMMIT loses the acknowledgment (`FATAL 57P01`) →
/// `CommitUncertain`, never a rollback claim; the pool then serves a NEW
/// backend PID. (The "Block timeout" note: "exercised in the Docker suite by
/// terminating the backend at COMMIT from a server-side trigger
/// (`pg_terminate_backend` in a deferred trigger), so the client's COMMIT
/// acknowledgment is lost".)
///
/// NOT the frozen cancellation-timing fixture
/// `transacted_cancel_after_commit_lost` — no pipeline cancellation is
/// delivered here (CANCEL_TOKEN is camel-core crate-private); that scenario
/// lives at the segment tier in `camel-core::transacted_segment`.
#[tokio::test(flavor = "multi_thread")]
async fn transacted_backend_kill_commit_uncertain() {
    let url = pg_url().await;
    let pool = new_pool(url, 4, ACQUIRE).await;
    run_ddl(
        &pool,
        &[
            "DROP TABLE IF EXISTS sqltrans_p2_rows CASCADE",
            "DROP FUNCTION IF EXISTS sqltrans_p2_kill() CASCADE",
            "CREATE TABLE sqltrans_p2_rows (id SERIAL PRIMARY KEY, note TEXT)",
            "CREATE OR REPLACE FUNCTION sqltrans_p2_kill() RETURNS trigger \
             LANGUAGE plpgsql AS $$ \
             BEGIN \
                 PERFORM pg_terminate_backend(pg_backend_pid()); \
                 RETURN NEW; \
             END $$",
            "CREATE CONSTRAINT TRIGGER sqltrans_p2_kill_trg \
             AFTER INSERT ON sqltrans_p2_rows \
             DEFERRABLE INITIALLY DEFERRED \
             FOR EACH ROW EXECUTE FUNCTION sqltrans_p2_kill()",
        ],
    )
    .await;

    let killed = Arc::new(StdMutex::new(None::<i32>));
    let mut segment = segment(
        &pool,
        ACQUIRE,
        SessionBody::with_pid_observer(
            vec!["INSERT INTO sqltrans_p2_rows (note) VALUES ('p2')"],
            &pool,
            Arc::clone(&killed),
        ),
    );
    let outcome = segment.run(exchange()).await;

    match outcome {
        PipelineOutcome::Failed(camel_api::CamelError::Transaction(
            TransactionError::CommitUncertain { datasource },
        )) => assert_eq!(datasource, DS, "CommitUncertain names the datasource"),
        other => panic!("expected CommitUncertain (never a rollback claim), got {other:?}"),
    }

    let killed_pid = killed
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .expect("the body must observe the backend before COMMIT kills it");

    assert_eq!(
        table_count(&pool, "sqltrans_p2_rows").await,
        0,
        "the commit aborted with the backend, so no row persists"
    );

    let replacement: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&pool)
        .await
        .expect("the pool must serve a replacement backend after the kill");
    assert_ne!(
        replacement, killed_pid,
        "the pool must serve a NEW backend pid after the killed one"
    );
}

// ── P3a ───────────────────────────────────────────────────────────────────

/// P3a: cancellation before the driver handoff → bounded rollback, no write.
///
/// The segment-level cancellation point (`CANCEL_TOKEN`, a camel-core
/// crate-private task-local) is not installable from this suite, so the
/// fixture pins the exact action the segment issues on that path: the
/// session's total `rollback()` before any COMMIT handoff.
#[tokio::test(flavor = "multi_thread")]
async fn transacted_cancel_before_commit() {
    let url = pg_url().await;
    let pool = new_pool(url, 4, ACQUIRE).await;
    run_ddl(
        &pool,
        &[
            "DROP TABLE IF EXISTS sqltrans_p3a_rows CASCADE",
            "CREATE TABLE sqltrans_p3a_rows (id SERIAL PRIMARY KEY, note TEXT)",
        ],
    )
    .await;

    let session = SqlTxSession::open_on_pool(
        DS,
        &pool,
        tokio::time::Instant::now() + BLOCK_DEADLINE,
        ACQUIRE,
    )
    .await
    .expect("session opens on a healthy pool");
    session.begin().await.expect("lazy BEGIN");
    session
        .exec_statement_for_test("INSERT INTO sqltrans_p3a_rows (note) VALUES ('pre-cutoff')")
        .await
        .expect("the pre-cutoff write is accepted inside the transaction");

    let started = std::time::Instant::now();
    let outcome = session.rollback().await.expect("bounded rollback");
    let elapsed = started.elapsed();

    assert_eq!(
        outcome,
        TransactionOutcome::RolledBack,
        "the pre-cutoff path rolls back"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "rollback must stay inside the 5 s finalize grace, took {elapsed:?}"
    );
    assert_eq!(
        table_count(&pool, "sqltrans_p3a_rows").await,
        0,
        "no partial write persists"
    );
}

// ── P3b ───────────────────────────────────────────────────────────────────

/// P3b: after an acknowledged COMMIT a stray ROLLBACK is a server-side no-op
/// and the committed row survives.
///
/// The post-cutoff invariant is observed on the committed side: the block
/// resolves `Completed`, and with `max_connections=1` the stray `ROLLBACK`
/// runs on the same backend that committed — PostgreSQL answers `Ok` with a
/// "no transaction in progress" warning, and the row persists.
///
/// NOT the frozen cancellation-timing fixture
/// `transacted_cancel_after_commit_ack` — no pipeline cancellation is
/// delivered here; that scenario lives at the segment tier in
/// `camel-core::transacted_segment`.
#[tokio::test(flavor = "multi_thread")]
async fn transacted_commit_ack_stray_rollback_noop() {
    let url = pg_url().await;
    let pool = new_pool(url, 1, ACQUIRE).await;
    run_ddl(
        &pool,
        &[
            "DROP TABLE IF EXISTS sqltrans_p3b_rows CASCADE",
            "CREATE TABLE sqltrans_p3b_rows (id SERIAL PRIMARY KEY, note TEXT)",
        ],
    )
    .await;

    let mut segment = segment(
        &pool,
        ACQUIRE,
        SessionBody::statements(vec![
            "INSERT INTO sqltrans_p3b_rows (note) VALUES ('post-ack')",
        ]),
    );
    let outcome = segment.run(exchange()).await;
    assert!(
        matches!(outcome, PipelineOutcome::Completed(_)),
        "an acknowledged COMMIT resolves the block as Completed, got {outcome:?}"
    );

    let mut conn = acquire_conn(&pool).await;
    sqlx::query("ROLLBACK")
        .execute(&mut *conn)
        .await
        .expect("a stray ROLLBACK after an acknowledged COMMIT is a server-side no-op");
    drop(conn);

    assert_eq!(
        table_count(&pool, "sqltrans_p3b_rows").await,
        1,
        "the committed row survives the stray ROLLBACK"
    );
}

// ── P4 ────────────────────────────────────────────────────────────────────

/// P4: admission hygiene discards a prior borrower's abandoned transaction
/// on the same backend, and the naive-`BEGIN` variant reproduces the
/// poisoning hazard that makes hygiene mandatory.
///
/// `max_connections=1` plus an explicit same-backend-PID assertion means a
/// pool replacement can never satisfy either half: the session and the naive
/// borrower provably run on the connection the abandoned transaction used.
#[tokio::test(flavor = "multi_thread")]
async fn transacted_prior_borrower_discard() {
    let url = pg_url().await;

    // ── Half 1: hygiene discards the abandoned write ─────────────────────
    let pool = new_pool(url, 1, ACQUIRE).await;
    run_ddl(
        &pool,
        &[
            "DROP TABLE IF EXISTS sqltrans_p4_rows CASCADE",
            "CREATE TABLE sqltrans_p4_rows (id SERIAL PRIMARY KEY, note TEXT)",
        ],
    )
    .await;

    let abandoned_pid = abandon_open_transaction(
        &pool,
        "BEGIN",
        "INSERT INTO sqltrans_p4_rows (note) VALUES ('abandoned')",
    )
    .await;

    let recycled_pid = current_backend_pid(&pool).await;
    assert_eq!(
        recycled_pid, abandoned_pid,
        "max_connections=1 must hand the next borrower the SAME backend \
         (a replacement cannot satisfy this fixture)"
    );

    let session = SqlTxSession::open_on_pool(
        DS,
        &pool,
        tokio::time::Instant::now() + BLOCK_DEADLINE,
        ACQUIRE,
    )
    .await
    .expect("session opens over the poisoned connection");
    session
        .begin()
        .await
        .expect("admission hygiene terminated the abandoned transaction");
    session
        .exec_statement_for_test("INSERT INTO sqltrans_p4_rows (note) VALUES ('block')")
        .await
        .expect("the block write is accepted");
    let outcome = session.commit().await.expect("COMMIT acknowledged");
    assert_eq!(outcome, TransactionOutcome::Committed);

    assert_eq!(
        scalar_i64(
            &pool,
            "SELECT COUNT(*) FROM sqltrans_p4_rows WHERE note = 'abandoned'",
        )
        .await,
        0,
        "the prior borrower's uncommitted write is discarded by hygiene"
    );
    assert_eq!(
        table_count(&pool, "sqltrans_p4_rows").await,
        1,
        "only the block's own row persists"
    );

    // ── Half 2: the naive-BEGIN poisoning hazard (the regression) ────────
    let naive_pool = new_pool(url, 1, ACQUIRE).await;
    run_ddl(
        &naive_pool,
        &[
            "DROP TABLE IF EXISTS sqltrans_p4_poison CASCADE",
            "CREATE TABLE sqltrans_p4_poison (id SERIAL PRIMARY KEY, note TEXT)",
        ],
    )
    .await;

    let abandoned_pid = abandon_open_transaction(
        &naive_pool,
        "BEGIN",
        "INSERT INTO sqltrans_p4_poison (note) VALUES ('prior-borrower')",
    )
    .await;

    let mut conn = acquire_conn(&naive_pool).await;
    let naive_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *conn)
        .await
        .expect("backend pid of the re-borrowed connection");
    assert_eq!(
        naive_pid, abandoned_pid,
        "the naive borrower provably reuses the abandoned transaction's backend"
    );

    // No hygiene: PostgreSQL accepts the nested BEGIN with a WARNING only, so
    // the server keeps the PRIOR transaction and our COMMIT commits it.
    for statement in [
        "BEGIN",
        "INSERT INTO sqltrans_p4_poison (note) VALUES ('naive-1')",
        "BEGIN",
        "INSERT INTO sqltrans_p4_poison (note) VALUES ('naive-2')",
        "COMMIT",
    ] {
        sqlx::query(statement)
            .execute(&mut *conn)
            .await
            .unwrap_or_else(|error| panic!("naive statement failed: {statement}: {error}"));
    }
    drop(conn);

    assert_eq!(
        table_count(&naive_pool, "sqltrans_p4_poison").await,
        3,
        "without hygiene the block's COMMIT commits the prior borrower's row: \
         the documented poisoning hazard that makes hygiene mandatory"
    );
}

/// Runs `statements` on one pooled connection (leaving any `BEGIN` open) and
/// returns that backend's PID before dropping the connection back into the
/// pool, transaction still open.
async fn abandon_open_transaction(pool: &AnyPool, begin: &str, insert: &str) -> i32 {
    let mut conn = acquire_conn(pool).await;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *conn)
        .await
        .expect("backend pid");
    for statement in [begin, insert] {
        sqlx::query(sqlx::AssertSqlSafe(statement.to_owned()))
            .execute(&mut *conn)
            .await
            .unwrap_or_else(|error| panic!("abandoned-tx statement failed: {statement}: {error}"));
    }
    // Dropping the `PoolConnection` returns it to the pool with the
    // transaction still open — exactly the prior-borrower state.
    drop(conn);
    pid
}

async fn current_backend_pid(pool: &AnyPool) -> i32 {
    sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(pool)
        .await
        .expect("backend pid")
}

// ── P5 ────────────────────────────────────────────────────────────────────

/// P5: pool exhaustion is bounded and typed — `PoolExhausted` names the
/// datasource, the configured `max_connections`, and the milliseconds spent
/// waiting.
#[tokio::test(flavor = "multi_thread")]
async fn transacted_pool_exhausted_typed() {
    let url = pg_url().await;
    let pool = new_pool(url, 1, Duration::from_millis(800)).await;
    install_drivers();

    // Hold the pool's only connection for the whole acquire bound.
    let held = acquire_conn(&pool).await;

    let started = std::time::Instant::now();
    let error = SqlTxSession::open_on_pool(
        DS,
        &pool,
        tokio::time::Instant::now() + BLOCK_DEADLINE,
        Duration::from_millis(800),
    )
    .await
    .err()
    .expect("acquiring a held max_connections=1 pool must time out");
    let elapsed = started.elapsed();

    match error {
        TransactionError::PoolExhausted {
            datasource,
            max_connections,
            waited_ms,
        } => {
            assert_eq!(datasource, DS, "PoolExhausted names the datasource");
            assert_eq!(max_connections, 1, "PoolExhausted names the pool size");
            assert!(
                (700..=5_000).contains(&waited_ms),
                "waited_ms is measured from the acquire start: {waited_ms}"
            );
        }
        other => panic!("expected typed PoolExhausted, got {other:?}"),
    }
    assert!(
        elapsed < Duration::from_secs(5),
        "exhaustion must be bounded by the acquire deadline, took {elapsed:?}"
    );

    drop(held);
    assert_eq!(
        scalar_i64(&pool, "SELECT 1").await,
        1,
        "the pool stays usable after the typed exhaustion"
    );
}

// ── P6 ────────────────────────────────────────────────────────────────────

/// P6: a concurrent admission on a session whose single execution slot is
/// held returns the typed `Busy` immediately — no await, no hang.
///
/// The slot is held by a genuinely running statement: the fixture waits until
/// PostgreSQL reports the `pg_sleep` backend active (`pg_stat_activity`) and
/// only then attempts the admission, so the held-slot state is observed, not
/// raced.
#[tokio::test(flavor = "multi_thread")]
async fn transacted_busy_slot_deterministic() {
    let url = pg_url().await;
    let pool = new_pool(url, 4, ACQUIRE).await;

    let session = Arc::new(
        SqlTxSession::open_on_pool(
            DS,
            &pool,
            tokio::time::Instant::now() + BLOCK_DEADLINE,
            ACQUIRE,
        )
        .await
        .expect("session opens"),
    );

    let holder = Arc::clone(&session);
    let handle = tokio::spawn(async move {
        holder
            .exec_statement_for_test("SELECT pg_sleep(2) /* sqltrans_p6_busy */")
            .await
    });

    wait_until(
        "sqltrans_p6 busy statement active on the server",
        Duration::from_secs(5),
        Duration::from_millis(20),
        || {
            let pool = pool.clone();
            async move {
                let active: i64 = sqlx::query_scalar(
                    "SELECT COUNT(*) FROM pg_stat_activity \
                     WHERE state = 'active' \
                       AND query LIKE '%sqltrans_p6_busy%'",
                )
                .fetch_one(&pool)
                .await
                .map_err(|error| error.to_string())?;
                Ok(active > 0)
            }
        },
    )
    .await
    .expect("the slot-holding statement never became active");

    let admitted = std::time::Instant::now();
    let error = session
        .begin()
        .await
        .expect_err("an admission on a held slot must fail typed, not await");
    assert!(
        matches!(&error, TransactionError::Busy { datasource } if datasource == DS),
        "expected typed Busy naming the datasource, got {error:?}"
    );
    assert!(
        admitted.elapsed() < Duration::from_secs(1),
        "Busy is decided by try_lock, never by awaiting the slot: took {:?}",
        admitted.elapsed()
    );

    // Release the slot deterministically before the session drops.
    handle.abort();
    let _ = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("the aborted slot-holder must finish promptly");
    drop(session);

    assert_eq!(
        scalar_i64(&pool, "SELECT 1").await,
        1,
        "the pool stays usable"
    );
}
