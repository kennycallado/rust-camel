//! Block-owned transaction session over a guarded pooled connection.
//!
//! [`SqlTxSession`] is the concrete [`TransactionSession`] implementation for
//! `transacted { datasource }` blocks: the sql component's `Arc::downcast`
//! target reached through the port's `Any` supertrait upcast. It is the sole
//! owner of its pooled connection (the connection is never stored in exchange
//! extensions), armed with a discard guard from checkout until an acknowledged
//! finalization: any phase except `Done`/`Detached` dropped out from under the
//! session detaches the connection synchronously (R0, `DiscardOnDrop`).
//!
//! The lifecycle keys on a private `TxPhase`, never on the frozen
//! [`TransactionState`]: "BEGIN bytes may be on the wire" and "COMMIT handed
//! off" drive the finalization decision but cannot be expressed by the port
//! enum (604c §A). `BEGIN` / `COMMIT` /
//! `ROLLBACK` are issued as plain statements on the borrowed
//! connection — no sqlx `Transaction` wrapper is ever stored, and the borrow
//! is confined to one await scope inside this module.
//!
//! Unit-tier testability without Docker: the driver-facing statement IO is
//! behind the private `TxIo` trait (the real backend is
//! `AnyConnIo` over an `AnyPool` connection; fixtures script a fake), and
//! the time source is behind `TxTimer` (the default `TokioTimer` runs on
//! tokio's clock, so `start_paused` tests get a virtual clock; an injected
//! timer replaces it). The camel-api port is untouched by both seams.
//!
//! Rulings: `.opencode/fleet/orders/604c-sqltrans-s2-ruling.md` (§A/§B) and
//! `604d-sqltrans-s2-ruling2.md` (total rollback contract). Binding
//! precedence: `openspec/changes/sqltrans/specs/sql-transactions/spec.md`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex as SyncMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use camel_api::transaction::{
    BeginFuture, FinalizeFuture, TransactionError, TransactionOutcome, TransactionSession,
    TransactionState,
};
use sqlx::Any;
use sqlx::AnyPool;
use sqlx::pool::PoolConnection;
use tokio::sync::Mutex as AsyncMutex;

/// Fixed finalize grace (604c §A): one absolute deadline,
/// `finalize_start + 5 s`, covering every finalization SQL in the run
/// (COMMIT plus any cleanup ROLLBACK). A seam parameter, not a second knob:
/// the open path always uses this constant.
const FINALIZE_GRACE: Duration = Duration::from_secs(5);

const SQL_BEGIN: &str = "BEGIN";
const SQL_COMMIT: &str = "COMMIT";
const SQL_ROLLBACK: &str = "ROLLBACK";

/// Private lifecycle phase behind the frozen port enum (604c §A).
///
/// `Clean`/`BeginPending`/`Open`/`Detached` are the phases finalization can
/// legally enter; only this module's begin/commit/rollback paths move the
/// phase into `*InFlight`/`Done`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TxPhase {
    /// Hygiene acknowledged, nothing sent.
    Clean,
    /// Set before the BEGIN write; cleared only by the ACK.
    BeginPending,
    /// BEGIN acknowledged.
    Open,
    /// Set immediately before the COMMIT write.
    CommitInFlight,
    /// ROLLBACK sent, not yet acknowledged.
    RollbackInFlight,
    /// Acknowledged finalization, guard disarmed.
    Done,
    /// Connection discarded; terminal.
    Detached,
}

/// Projects [`TxPhase`] onto the frozen [`TransactionState`], row for row
/// per the 604c §A table (`BeginPending → Fresh` is lossy by design: nothing
/// may decide on `state()`).
#[must_use]
fn project(phase: TxPhase) -> TransactionState {
    match phase {
        TxPhase::Clean | TxPhase::BeginPending => TransactionState::Fresh,
        TxPhase::Open => TransactionState::Active,
        TxPhase::CommitInFlight | TxPhase::RollbackInFlight => TransactionState::Finalizing,
        TxPhase::Done => TransactionState::Done,
        TxPhase::Detached => TransactionState::Detached,
    }
}

/// Driver-neutral severity: only the PG driver's exact `ERROR` severity
/// counts as affirmative rejection evidence (604c R7); `FATAL`/`PANIC`
/// (57P01), warnings/notices, and unmapped drivers are not evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DbSeverity {
    Error,
    Other,
}

/// Affirmative database error, driver-neutral: severity, SQLSTATE, message.
#[derive(Debug, Clone)]
pub(crate) struct DbAnswer {
    severity: DbSeverity,
    code: Option<String>,
    message: String,
}

/// Statement failure seen by the session.
#[derive(Debug, Clone)]
pub(crate) enum TxIoFailure {
    /// The server answered with an affirmative database error.
    Database(DbAnswer),
    /// Transport/protocol/pool failure or a connection with no driver
    /// mapping: the statement's outcome is unknown.
    Lost,
}

/// R7 disposition of a failed COMMIT.
enum CommitDisposition {
    /// Affirmative rejection: severity exactly `ERROR` AND SQLSTATE class
    /// `23` (integrity) or `40` (transaction abort — the server affirms the
    /// transaction is over).
    Rejected {
        /// Driver-reported detail carried into `CommitFailed`.
        reason: String,
    },
    /// Everything else: `FATAL`/`PANIC` severity (57P01), other classes,
    /// Io, Protocol, PoolClosed — uncertain, detach, no rollback claim.
    Lost,
}

/// Classifies a COMMIT failure per the R7 rows.
#[must_use]
fn commit_disposition(failure: &TxIoFailure) -> CommitDisposition {
    let TxIoFailure::Database(answer) = failure else {
        return CommitDisposition::Lost;
    };
    let reject_class = answer
        .code
        .as_deref()
        .is_some_and(|code| code.starts_with("23") || code.starts_with("40"));
    if answer.severity == DbSeverity::Error && reject_class {
        CommitDisposition::Rejected {
            reason: match answer.code.as_deref() {
                Some(code) => format!("SQLSTATE {code}: {}", answer.message),
                None => answer.message.clone(),
            },
        }
    } else {
        CommitDisposition::Lost
    }
}

/// Boxed one-statement execution against the guarded connection.
type ExecFuture<'a> = Pin<Box<dyn Future<Output = Result<(), TxIoFailure>> + Send + 'a>>;

/// Driver-facing statement IO for one checked-out connection.
///
/// The real backend ([`AnyConnIo`]) wraps the pooled `AnyConnection`; the
/// unit fixtures script a fake. This seam is what keeps the session's phase
/// machine unit-tier testable without Docker; the camel-api port stays
/// untouched.
pub(crate) trait TxIo: Send + 'static {
    /// Execute one plain statement (`BEGIN`/`COMMIT`/`ROLLBACK`) on the
    /// guarded connection. The returned future must record the statement only
    /// when actually polled, so a future dropped by a deadline race counts as
    /// never issued.
    fn exec(&mut self, sql: &'static str) -> ExecFuture<'_>;

    /// Detach synchronously: close/discard the connection, never return it
    /// to the pool, send no SQL (R0/R3 semantics).
    fn detach(&mut self);

    /// Return the connection to the pool (guard disarmed). Consumes the
    /// backend.
    fn release(self: Box<Self>);
}

/// Real backend: sole owner of one pooled `AnyConnection`.
struct AnyConnIo {
    conn: Option<PoolConnection<Any>>,
}

impl TxIo for AnyConnIo {
    fn exec(&mut self, sql: &'static str) -> ExecFuture<'_> {
        Box::pin(async move {
            let Some(conn) = self.conn.as_mut() else {
                return Err(TxIoFailure::Lost);
            };
            match sqlx::query(sqlx::AssertSqlSafe(sql))
                .execute(&mut **conn)
                .await
            {
                Ok(_) => Ok(()),
                Err(error) => Err(to_io_failure(&error)),
            }
        })
    }

    fn detach(&mut self) {
        // Detach first so the pool never sees this connection again; dropping
        // the detached handle closes it (same convention as the producer's
        // `DiscardOnDrop` guard).
        if let Some(conn) = self.conn.take() {
            drop(conn.detach());
        }
    }

    fn release(self: Box<Self>) {
        // Dropping the `PoolConnection` returns it to the pool.
        drop(self);
    }
}

/// Placeholder left in the admission slot once the real backend left it
/// (released or detached): every statement on it fails lost, so latent misuse
/// can never touch a connection that no longer belongs to the session.
struct DetachedIo;

impl TxIo for DetachedIo {
    fn exec(&mut self, _sql: &'static str) -> ExecFuture<'_> {
        Box::pin(std::future::ready(Err(TxIoFailure::Lost)))
    }

    fn detach(&mut self) {}

    fn release(self: Box<Self>) {}
}

/// Checkout guard: sole owner of the checked-out backend from the instant
/// `pool.acquire()` resolves until the session takes ownership.
///
/// R0 arms AT checkout (frozen spec: "The discard guard is armed on
/// connection checkout"): dropping this guard — the open future dying
/// mid-hygiene, any error path in `open` — detaches synchronously, so a
/// connection whose statement outcome is unknown is never returned to the
/// pool. The guard is disarmed exactly on the `with_io` hand-off, where the
/// session's own [`Drop`] guard takes over.
struct CheckedOutBackend {
    io: Option<Box<dyn TxIo>>,
}

impl CheckedOutBackend {
    /// Arms the guard over the freshly checked-out backend.
    fn arm(io: Box<dyn TxIo>) -> Self {
        Self { io: Some(io) }
    }

    /// Hands the backend to the guard-bearing session; the guard no longer
    /// owns it. Only called after the admission hygiene acknowledged.
    fn disarm(mut self) -> Box<dyn TxIo> {
        match self.io.take() {
            Some(io) => io,
            None => {
                // Unreachable for a correct hand-off (the slot holds the
                // backend until this call), but the session must fail
                // closed rather than panic: the lost placeholder makes
                // every statement error and the guard still disarms.
                // log-policy: system-broken
                tracing::error!(
                    "checked-out backend missing at the with_io hand-off; \
                     the session continues on the lost placeholder"
                );
                Box::new(DetachedIo)
            }
        }
    }
}

impl TxIo for CheckedOutBackend {
    fn exec(&mut self, sql: &'static str) -> ExecFuture<'_> {
        let Some(io) = self.io.as_mut() else {
            return Box::pin(std::future::ready(Err(TxIoFailure::Lost)));
        };
        io.exec(sql)
    }

    fn detach(&mut self) {
        // Delegates to the backend (idempotent): the slot empties, so the
        // guard's own Drop stays a no-op.
        if let Some(mut io) = self.io.take() {
            io.detach();
        }
    }

    fn release(mut self: Box<Self>) {
        if let Some(io) = self.io.take() {
            io.release();
        }
    }
}

impl Drop for CheckedOutBackend {
    fn drop(&mut self) {
        if let Some(mut io) = self.io.take() {
            io.detach();
        }
    }
}

/// Maps a sqlx error onto the driver-neutral failure.
fn to_io_failure(error: &sqlx::Error) -> TxIoFailure {
    match error {
        sqlx::Error::Database(db) => TxIoFailure::Database(DbAnswer {
            severity: db_severity(&**db),
            code: db.code().as_deref().map(str::to_owned),
            message: db.message().to_owned(),
        }),
        // Io, Protocol, PoolClosed, PoolTimedOut, WorkerCrashed, ...: the
        // outcome is unknown.
        _ => TxIoFailure::Lost,
    }
}

/// PG severity mapping, fail-closed: only the PG driver's exact `ERROR`
/// severity counts as [`DbSeverity::Error`] (604c R7 names 57P01 — a
/// `FATAL` — as the `CommitUncertain` shape, so severity must be exact).
#[cfg(feature = "postgres")]
fn db_severity(db: &dyn sqlx::error::DatabaseError) -> DbSeverity {
    match db.try_downcast_ref::<sqlx::postgres::PgDatabaseError>() {
        Some(pg) if pg.severity() == sqlx::postgres::PgSeverity::Error => DbSeverity::Error,
        _ => DbSeverity::Other,
    }
}

/// Non-PG builds cannot prove severity: not affirmative evidence.
#[cfg(not(feature = "postgres"))]
fn db_severity(_db: &dyn sqlx::error::DatabaseError) -> DbSeverity {
    DbSeverity::Other
}

/// Time source for the session's deadlines (absolute instants per the
/// spec's "Deadlines are compared as absolute instants" clause).
///
/// The default [`TokioTimer`] runs on tokio's clock: real when unpaused and
/// fully virtual under `#[tokio::test(start_paused = true)]`. Tests may
/// inject any other [`TxTimer`] through
/// [`SqlTxSession::with_io`].
pub(crate) trait TxTimer: Send + Sync + 'static {
    /// Absolute now.
    fn now(&self) -> tokio::time::Instant;

    /// Sleep until the absolute instant.
    fn sleep_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}

/// Default timer on tokio's clock.
struct TokioTimer;

impl TxTimer for TokioTimer {
    fn now(&self) -> tokio::time::Instant {
        tokio::time::Instant::now()
    }

    fn sleep_until(
        &self,
        deadline: tokio::time::Instant,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep_until(deadline))
    }
}

/// Outcome of one deadline-raced statement.
#[derive(Debug)]
enum StmtOutcome {
    /// The driver acknowledged the statement.
    Ack,
    /// The deadline fired before the driver answered.
    Deadline,
    /// The driver answered with an error.
    Failed(TxIoFailure),
}

/// Executes one plain statement raced against an absolute deadline.
///
/// Biased with the deadline arm first, so a tie resolves like the ruling's
/// tie rule (deadline wins). The losing future is dropped by the race; the
/// connection state is then unknown and every caller detaches or reports
/// uncertainty accordingly.
async fn run_statement(
    io: &mut dyn TxIo,
    sql: &'static str,
    deadline: tokio::time::Instant,
    timer: &dyn TxTimer,
) -> StmtOutcome {
    tokio::select! {
        biased;
        _ = timer.sleep_until(deadline) => StmtOutcome::Deadline,
        result = io.exec(sql) => match result {
            Ok(()) => StmtOutcome::Ack,
            Err(failure) => StmtOutcome::Failed(failure),
        },
    }
}

/// Milliseconds elapsed on the injected clock since `since`.
fn elapsed_ms(timer: &dyn TxTimer, since: tokio::time::Instant) -> u64 {
    u64::try_from(timer.now().saturating_duration_since(since).as_millis()).unwrap_or(u64::MAX)
}

/// One block-owned transaction on one datasource.
///
/// The session owns its pooled connection as sole owner through the
/// admission slot (a `tokio::sync::Mutex` around the `TxIo` backend: the
/// slot IS the one-statement-at-a-time admission, `try_lock` → `Busy`), the
/// discard guard is that same ownership armed from checkout — dropping the
/// session in any phase except `Done`/`Detached` detaches synchronously
/// (R0). `rollback_only` is the port's atomic flag.
///
/// Constructed through `open` (checkout + admission hygiene
/// → `Clean`) by the datasource manager; unit fixtures construct directly
/// over a scripted backend via `with_io`.
pub struct SqlTxSession {
    datasource: Arc<str>,
    /// Admission slot: `try_lock` → one statement at a time; holds the
    /// guarded connection backend.
    slot: AsyncMutex<Box<dyn TxIo>>,
    /// Private phase; short critical sections, never held across an await.
    phase: SyncMutex<TxPhase>,
    /// The port's rollback-only flag (req 4: handled joined DB errors).
    rollback_only: AtomicBool,
    /// Injected time source.
    timer: Arc<dyn TxTimer>,
    /// Bound for the lazy BEGIN (604c §B.2: `begin_start + acquire_deadline`).
    acquire_deadline: Duration,
    /// Finalize grace: `finalize_start + finalize_grace` covers every
    /// finalization SQL in the run.
    finalize_grace: Duration,
    /// Pool size for `PoolExhausted` naming.
    max_connections: u32,
}

impl SqlTxSession {
    /// Builds a session in [`TxPhase::Clean`] over an already-checked-out,
    /// hygiene-acknowledged backend.
    ///
    /// `finalize_grace` is a seam for virtual-clock fixtures; the open path
    /// always uses [`FINALIZE_GRACE`].
    pub(crate) fn with_io(
        datasource: impl Into<Arc<str>>,
        io: Box<dyn TxIo>,
        timer: Arc<dyn TxTimer>,
        acquire_deadline: Duration,
        finalize_grace: Duration,
        max_connections: u32,
    ) -> Self {
        Self {
            datasource: datasource.into(),
            slot: AsyncMutex::new(io),
            phase: SyncMutex::new(TxPhase::Clean),
            rollback_only: AtomicBool::new(false),
            timer,
            acquire_deadline,
            finalize_grace,
            max_connections,
        }
    }

    /// Admission path for the datasource manager's `open_session`: check out
    /// a pooled connection, run the hygiene `ROLLBACK` exactly once, land in
    /// [`TxPhase::Clean`].
    ///
    /// Checkout and hygiene race the same absolute instants —
    /// `min(block_abs, open_start + acquire_deadline)` (604c admission) — and
    /// the winner is decided by comparing those instants, never by polling
    /// order: `PoolExhausted` iff `acquire_abs < block_abs` strictly, else
    /// `Timeout` (tie included). The guard arms at checkout: the backend owns
    /// the connection from the moment the checkout future resolves, so a
    /// failure after that detaches. Exactly one `ROLLBACK` attempt is made
    /// even when a deadline fires (the race drops the in-flight future).
    /// `BEGIN` is never sent here.
    pub(crate) async fn open(
        datasource: impl Into<Arc<str>>,
        pool: &AnyPool,
        block_abs: tokio::time::Instant,
        acquire_deadline: Duration,
        timer: Arc<dyn TxTimer>,
    ) -> Result<Self, TransactionError> {
        let datasource = datasource.into();
        let max_connections = crate::pool_factory::pool_max_connections(pool);
        let open_start = timer.now();
        let acquire_abs = open_start + acquire_deadline;

        let conn = tokio::select! {
            biased;
            _ = timer.sleep_until(block_abs) => {
                // Frozen spec "Block timeout": deadlines are compared as
                // absolute instants, never by polling order. When this arm
                // wins while the acquire deadline is strictly earlier, both
                // instants have passed at the same poll and the admission
                // reports `PoolExhausted`; the tie and any block-first
                // ordering report `Timeout`.
                if acquire_abs < block_abs {
                    return Err(TransactionError::PoolExhausted {
                        datasource: datasource.to_string(),
                        max_connections,
                        waited_ms: elapsed_ms(timer.as_ref(), open_start),
                    });
                }
                return Err(TransactionError::Timeout {
                    datasource: datasource.to_string(),
                    // Elapsed time since the block entry (open start), not
                    // since the deadline: the typed error names datasource
                    // and elapsed time (frozen spec "Block timeout").
                    elapsed_ms: elapsed_ms(timer.as_ref(), open_start),
                });
            }
            _ = timer.sleep_until(acquire_abs) => {
                return Err(TransactionError::PoolExhausted {
                    datasource: datasource.to_string(),
                    max_connections,
                    waited_ms: elapsed_ms(timer.as_ref(), open_start),
                });
            }
            acquired = pool.acquire() => {
                acquired.map_err(|_| TransactionError::PoolExhausted {
                    datasource: datasource.to_string(),
                    max_connections,
                    waited_ms: elapsed_ms(timer.as_ref(), open_start),
                })?
            }
        };

        // R0 arms AT checkout (frozen spec: "The discard guard is armed on
        // connection checkout"): from this instant the guard owns the
        // connection, so a dropped open future mid-hygiene detaches instead
        // of re-pooling. The guard disarms exactly at the `with_io` hand-off
        // inside [`Self::admit`].
        let backend = CheckedOutBackend::arm(Box::new(AnyConnIo { conn: Some(conn) }));
        Self::admit(
            datasource,
            backend,
            block_abs,
            acquire_abs,
            acquire_deadline,
            max_connections,
            timer,
        )
        .await
    }

    /// Admission tail shared by [`SqlTxSession::open`] and the unit fixtures:
    /// hygiene `ROLLBACK` against the already-armed checkout guard, then the
    /// disarm into the guard-bearing session. Crate-visible so the fixtures
    /// can drive the checkout guard over a scripted backend.
    async fn admit(
        datasource: Arc<str>,
        backend: CheckedOutBackend,
        block_abs: tokio::time::Instant,
        acquire_abs: tokio::time::Instant,
        acquire_deadline: Duration,
        max_connections: u32,
        timer: Arc<dyn TxTimer>,
    ) -> Result<Self, TransactionError> {
        let mut backend = backend;
        admission_hygiene(
            &mut backend,
            Arc::clone(&timer),
            &datasource,
            max_connections,
            block_abs,
            acquire_abs,
        )
        .await?;

        Ok(Self::with_io(
            datasource,
            backend.disarm(),
            timer,
            acquire_deadline,
            FINALIZE_GRACE,
            max_connections,
        ))
    }

    /// Manager entry point over a real pool: `open` on the
    /// default tokio clock.
    ///
    /// The injectable-clock `open` stays crate-private for the
    /// unit fixtures; this wrapper is the constructor the datasource manager
    /// (sqltrans-exec step 9) and the PG integration suite
    /// (`crates/camel-test/tests/sql_transacted_test.rs`) drive.
    pub async fn open_on_pool(
        datasource: impl Into<Arc<str>>,
        pool: &AnyPool,
        block_abs: tokio::time::Instant,
        acquire_deadline: Duration,
    ) -> Result<Self, TransactionError> {
        Self::open(
            datasource,
            pool,
            block_abs,
            acquire_deadline,
            Arc::new(TokioTimer),
        )
        .await
    }

    /// Test-support statement execution on the session's own connection: the
    /// body-side counterpart of a joined `sql:` step once the executor lends
    /// this connection to the producer (sqltrans-exec step 9).
    ///
    /// One plain statement through the same admission slot the block owns —
    /// an overlapping admission observes the held-slot arm — and never
    /// finalizes the transaction. The PG integration fixtures use it to put
    /// rows inside the session's transaction before the segment commits or
    /// rolls back; the executor's lent-connection path is the supported
    /// surface, not this helper.
    ///
    /// Returns the driver message on failure: the fixtures assert a
    /// statement's success, not a typed session outcome.
    ///
    /// `#[doc(hidden)]`: remove when the producer lends the session
    /// connection (executor slice).
    #[doc(hidden)]
    pub async fn exec_statement_for_test(&self, sql: &'static str) -> Result<(), String> {
        let Ok(mut slot) = self.slot.try_lock() else {
            return Err(TransactionError::Busy {
                datasource: self.datasource.to_string(),
            }
            .to_string());
        };
        match slot.exec(sql).await {
            Ok(()) => Ok(()),
            Err(TxIoFailure::Database(answer)) => Err(format!(
                "statement `{sql}` failed: SQLSTATE {}: {}",
                answer.code.as_deref().unwrap_or("-"),
                answer.message
            )),
            Err(TxIoFailure::Lost) => Err(format!(
                "statement `{sql}` lost the connection (outcome unknown)"
            )),
        }
    }

    fn phase(&self) -> TxPhase {
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn store_phase(&self, phase: TxPhase) {
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = phase;
    }

    /// Detaches through the slot: connection closed/discarded, never
    /// re-pooled, no SQL; phase → `Detached` (terminal).
    fn detach_slot(&self, slot: &mut tokio::sync::MutexGuard<'_, Box<dyn TxIo>>) {
        slot.detach();
        let _ = std::mem::replace(&mut **slot, Box::new(DetachedIo));
        self.store_phase(TxPhase::Detached);
    }

    /// `begin()` per 604c §B, verbatim.
    ///
    /// The slot lock orders every begin state transition: the phase is judged
    /// INSIDE the lock, so a concurrent begin that detaches (§B.4/§B.5)
    /// between this call's admission and its write can never be overwritten —
    /// `Detached` is terminal and is never transitioned from (§B.1).
    async fn begin_inner(&self) -> Result<(), TransactionError> {
        let datasource = self.datasource.to_string();
        let Ok(mut slot) = self.slot.try_lock() else {
            // §B.1 requires the slot to judge the phase; a held slot means
            // another begin/statement is mid-flight — the frozen Busy row.
            return Err(TransactionError::Busy { datasource });
        };
        match self.phase() {
            TxPhase::Open => return Ok(()),
            TxPhase::Clean => {}
            _ => return Err(TransactionError::SessionAborted { datasource }),
        }
        // §A/§B: BeginPending goes up BEFORE the BEGIN write and is cleared
        // only by the ACK, so a dropped future stays BeginPending (R3/R0).
        self.store_phase(TxPhase::BeginPending);
        let begin_start = self.timer.now();
        let deadline = begin_start + self.acquire_deadline;
        match run_statement(&mut **slot, SQL_BEGIN, deadline, self.timer.as_ref()).await {
            StmtOutcome::Ack => {
                self.store_phase(TxPhase::Open);
                Ok(())
            }
            StmtOutcome::Deadline => {
                // §B.4: own timeout → detach, `PoolExhausted`; NO cleanup SQL.
                self.detach_slot(&mut slot);
                Err(TransactionError::PoolExhausted {
                    datasource,
                    max_connections: self.max_connections,
                    waited_ms: elapsed_ms(self.timer.as_ref(), begin_start),
                })
            }
            StmtOutcome::Failed(_) => {
                // §B.5: any driver error → detach, `RollbackUncertain`; NO
                // cleanup SQL (protocol position unknown, nothing durable).
                self.detach_slot(&mut slot);
                Err(TransactionError::RollbackUncertain { datasource })
            }
        }
    }

    /// `commit()` per 604d + R7.
    async fn commit_inner(&self) -> Result<TransactionOutcome, TransactionError> {
        let datasource = self.datasource.to_string();
        // Port contract: precondition `Active`; otherwise no SQL.
        if self.phase() != TxPhase::Open {
            return Err(TransactionError::SessionAborted { datasource });
        }
        let Ok(mut slot) = self.slot.try_lock() else {
            // Unreachable for a correct owner (finalization happens after the
            // body); a held slot means the commit cannot be verified.
            // log-policy: system-broken
            tracing::error!(
                datasource = %self.datasource,
                "commit while the session slot is held; the commit outcome cannot be verified"
            );
            return Err(TransactionError::CommitUncertain { datasource });
        };
        let finalize_start = self.timer.now();
        let grace_deadline = finalize_start + self.finalize_grace;
        // R7: handed-off marker set immediately before the COMMIT write.
        // After the handoff the block deadline and cancellation are never
        // consulted — only this finalize grace applies.
        self.store_phase(TxPhase::CommitInFlight);
        match run_statement(&mut **slot, SQL_COMMIT, grace_deadline, self.timer.as_ref()).await {
            StmtOutcome::Ack => {
                self.store_phase(TxPhase::Done);
                release_to_pool(&mut slot);
                Ok(TransactionOutcome::Committed)
            }
            StmtOutcome::Deadline => {
                // Grace elapsed: uncertain, detach, no ROLLBACK, no claim.
                self.detach_slot(&mut slot);
                Err(TransactionError::CommitUncertain { datasource })
            }
            StmtOutcome::Failed(failure) => match commit_disposition(&failure) {
                CommitDisposition::Lost => {
                    // R7: FATAL/PANIC severity (57P01), other SQLSTATE
                    // classes, Io, Protocol, PoolClosed — detach immediately;
                    // no ROLLBACK, no rollback claim.
                    self.detach_slot(&mut slot);
                    Err(TransactionError::CommitUncertain { datasource })
                }
                CommitDisposition::Rejected { reason } => {
                    // ONE cleanup ROLLBACK within the remaining grace (a
                    // no-op WARNING on PG). The frozen spec WINS over 604c
                    // R7's sub-row: a COMMIT rejected by the database SHALL
                    // fail with the typed `CommitFailed` in BOTH cleanup
                    // outcomes — the cleanup uncertainty stays observable
                    // through the warn! log and the `Detached` state and
                    // never replaces the primary.
                    match run_statement(
                        &mut **slot,
                        SQL_ROLLBACK,
                        grace_deadline,
                        self.timer.as_ref(),
                    )
                    .await
                    {
                        StmtOutcome::Ack => {
                            self.store_phase(TxPhase::Done);
                            release_to_pool(&mut slot);
                            // 604d: the session returns only the cleanup
                            // result or the typed commit result — the
                            // segment owns primary-vs-cleanup separation.
                            Err(TransactionError::CommitFailed { datasource, reason })
                        }
                        StmtOutcome::Deadline | StmtOutcome::Failed(_) => {
                            self.detach_slot(&mut slot);
                            // log-policy: cleanup-uncertainty — logged, never
                            // substituted for the primary.
                            tracing::warn!(
                                datasource = %self.datasource,
                                "COMMIT was rejected and the cleanup ROLLBACK was \
                                 not acknowledged; the connection is detached and \
                                 the typed CommitFailed stands"
                            );
                            Err(TransactionError::CommitFailed { datasource, reason })
                        }
                    }
                }
            },
        }
    }

    /// Total `rollback()` per the 604d contract; the session selects the
    /// action from its private phase (R1–R3 + R4/R5 rollback legs + the
    /// defensive fallback).
    async fn rollback_inner(&self) -> Result<TransactionOutcome, TransactionError> {
        let datasource = self.datasource.to_string();
        match self.phase() {
            TxPhase::Clean => {
                // R2: BEGIN never attempted — disarm, return the connection
                // clean, no SQL; nothing was begun, so nothing is durable.
                debug_assert!(
                    !self.is_rollback_only(),
                    "R2 clean rollback with rollback_only set: the flag is unreachable in Clean"
                );
                let Ok(mut slot) = self.slot.try_lock() else {
                    // log-policy: system-broken
                    tracing::error!(
                        datasource = %self.datasource,
                        "clean rollback while the session slot is held; the connection cannot be returned"
                    );
                    return Err(TransactionError::RollbackUncertain { datasource });
                };
                self.store_phase(TxPhase::Done);
                release_to_pool(&mut slot);
                Ok(TransactionOutcome::RolledBack)
            }
            TxPhase::BeginPending => {
                // R3: protocol position unknown — detach, no SQL.
                let Ok(mut slot) = self.slot.try_lock() else {
                    // log-policy: system-broken
                    tracing::error!(
                        datasource = %self.datasource,
                        "rollback while the session slot is held; the connection cannot be detached"
                    );
                    return Err(TransactionError::RollbackUncertain { datasource });
                };
                self.detach_slot(&mut slot);
                Err(TransactionError::RollbackUncertain { datasource })
            }
            TxPhase::Open => {
                let Ok(mut slot) = self.slot.try_lock() else {
                    // log-policy: system-broken
                    tracing::error!(
                        datasource = %self.datasource,
                        "rollback while the session slot is held; the connection cannot be rolled back"
                    );
                    return Err(TransactionError::RollbackUncertain { datasource });
                };
                let finalize_start = self.timer.now();
                let grace_deadline = finalize_start + self.finalize_grace;
                self.store_phase(TxPhase::RollbackInFlight);
                match run_statement(
                    &mut **slot,
                    SQL_ROLLBACK,
                    grace_deadline,
                    self.timer.as_ref(),
                )
                .await
                {
                    StmtOutcome::Ack => {
                        self.store_phase(TxPhase::Done);
                        release_to_pool(&mut slot);
                        Ok(TransactionOutcome::RolledBack)
                    }
                    StmtOutcome::Deadline | StmtOutcome::Failed(_) => {
                        // Not acknowledged / stalled past grace: detach.
                        self.detach_slot(&mut slot);
                        Err(TransactionError::RollbackUncertain { datasource })
                    }
                }
            }
            TxPhase::Detached => {
                // R1: already detached — no SQL.
                Err(TransactionError::RollbackUncertain { datasource })
            }
            TxPhase::CommitInFlight | TxPhase::RollbackInFlight | TxPhase::Done => {
                // Defensive fallback (604c totality note): no SQL, and the
                // return must NOT claim detachment happened.
                // log-policy: system-broken
                tracing::error!(
                    datasource = %self.datasource,
                    "defensive finalization entry: session is already finalizing or done; \
                     no SQL sent, no detachment performed"
                );
                Err(TransactionError::RollbackUncertain { datasource })
            }
        }
    }
}

/// Consumes the slot's backend, returning the connection to the pool.
fn release_to_pool(slot: &mut tokio::sync::MutexGuard<'_, Box<dyn TxIo>>) {
    let io = std::mem::replace(&mut **slot, Box::new(DetachedIo));
    io.release();
}

/// Admission hygiene: ONE `ROLLBACK` on the freshly checked-out connection,
/// raced against the same absolute instants as the checkout.
///
/// A clean connection answers the hygiene as a safe no-op (probe P4); any
/// abandoned transaction on it is terminated, so a prior borrower's writes
/// can never be committed by this block. Unacknowledged hygiene detaches the
/// connection without sending `BEGIN`.
async fn admission_hygiene(
    io: &mut dyn TxIo,
    timer: Arc<dyn TxTimer>,
    datasource: &str,
    max_connections: u32,
    block_abs: tokio::time::Instant,
    acquire_abs: tokio::time::Instant,
) -> Result<(), TransactionError> {
    let hygiene_start = timer.now();
    let result = tokio::select! {
        biased;
        _ = timer.sleep_until(block_abs) => {
            // Absolute-instant comparison (frozen spec "Block timeout"): a
            // strictly-earlier acquire deadline that also elapsed at this
            // poll reports `PoolExhausted`; the tie and block-first report
            // `Timeout`.
            if acquire_abs < block_abs {
                Err(TransactionError::PoolExhausted {
                    datasource: datasource.to_owned(),
                    max_connections,
                    waited_ms: elapsed_ms(timer.as_ref(), hygiene_start),
                })
            } else {
                Err(TransactionError::Timeout {
                    datasource: datasource.to_owned(),
                    // Elapsed time since the hygiene start, not since the
                    // deadline: the typed error names datasource and elapsed
                    // time.
                    elapsed_ms: elapsed_ms(timer.as_ref(), hygiene_start),
                })
            }
        }
        _ = timer.sleep_until(acquire_abs) => Err(TransactionError::PoolExhausted {
            datasource: datasource.to_owned(),
            max_connections,
            waited_ms: elapsed_ms(timer.as_ref(), hygiene_start),
        }),
        ack = io.exec(SQL_ROLLBACK) => match ack {
            Ok(()) => Ok(()),
            Err(_) => Err(TransactionError::RollbackUncertain {
                datasource: datasource.to_owned(),
            }),
        },
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) => {
            io.detach();
            Err(error)
        }
    }
}

impl Drop for SqlTxSession {
    fn drop(&mut self) {
        // R0 (604c §A): forced drop in any phase except Done/Detached —
        // detach synchronously (connection closed/detached, never returned to
        // the pool), no SQL. This is the guard invariant, not a cooperative
        // row. No session future can hold the slot here: the port's futures
        // borrow the session (or hold an Arc clone of it), so none outlives
        // this drop.
        if matches!(self.phase(), TxPhase::Done | TxPhase::Detached) {
            return;
        }
        match self.slot.try_lock() {
            Ok(mut slot) => {
                slot.detach();
                let _ = std::mem::replace(&mut *slot, Box::new(DetachedIo));
            }
            Err(_) => {
                // log-policy: system-broken
                tracing::error!(
                    datasource = %self.datasource,
                    "session dropped while its statement slot is held; \
                     the connection cannot be detached from here"
                );
            }
        }
    }
}

impl TransactionSession for SqlTxSession {
    fn datasource(&self) -> &str {
        &self.datasource
    }

    fn state(&self) -> TransactionState {
        project(self.phase())
    }

    fn begin(&self) -> BeginFuture<'_> {
        Box::pin(self.begin_inner())
    }

    fn set_rollback_only(&self) {
        self.rollback_only.store(true, Ordering::Relaxed);
    }

    fn is_rollback_only(&self) -> bool {
        self.rollback_only.load(Ordering::Relaxed)
    }

    fn commit(&self) -> FinalizeFuture<'_> {
        Box::pin(self.commit_inner())
    }

    fn rollback(&self) -> FinalizeFuture<'_> {
        Box::pin(self.rollback_inner())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex as SyncMutex;
    use std::task::{Context, Poll, Waker};

    use super::*;
    use futures::task::noop_waker;

    const ACQUIRE: Duration = Duration::from_millis(50);
    const GRACE: Duration = Duration::from_secs(5);
    const MAX_CONNECTIONS: u32 = 4;

    /// Scripted driver answer per statement text; unscripted statements ack.
    #[derive(Debug, Clone)]
    enum Scripted {
        Ack,
        /// The statement never completes (stalls past any deadline).
        Pending,
        Fail(TxIoFailure),
    }

    /// Observable fake-driver state shared with the fixture.
    #[derive(Debug, Default)]
    struct FakeShared {
        responses: HashMap<&'static str, Scripted>,
        issued: Vec<&'static str>,
        detached: bool,
        released: bool,
    }

    /// Fake [`TxIo`] backend driven by a shared script.
    struct FakeIo {
        shared: Arc<SyncMutex<FakeShared>>,
    }

    impl FakeIo {
        fn channel() -> (Box<dyn TxIo>, Arc<SyncMutex<FakeShared>>) {
            let shared = Arc::new(SyncMutex::new(FakeShared::default()));
            (
                Box::new(FakeIo {
                    shared: Arc::clone(&shared),
                }),
                shared,
            )
        }

        fn script(sql: &'static str, answer: Scripted, shared: &SyncMutex<FakeShared>) {
            shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .responses
                .insert(sql, answer);
        }
    }

    impl TxIo for FakeIo {
        fn exec(&mut self, sql: &'static str) -> ExecFuture<'_> {
            Box::pin(async move {
                // Record on first poll: a future dropped by a deadline race
                // counts as never issued.
                self.shared
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .issued
                    .push(sql);
                let scripted = self
                    .shared
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .responses
                    .get(&sql)
                    .cloned()
                    .unwrap_or(Scripted::Ack);
                match scripted {
                    Scripted::Ack => Ok(()),
                    Scripted::Pending => std::future::pending().await,
                    Scripted::Fail(failure) => Err(failure),
                }
            })
        }

        fn detach(&mut self) {
            self.shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .detached = true;
        }

        fn release(self: Box<Self>) {
            self.shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .released = true;
        }
    }

    fn issued_of(shared: &SyncMutex<FakeShared>) -> Vec<&'static str> {
        shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .issued
            .clone()
    }

    fn is_detached(shared: &SyncMutex<FakeShared>) -> bool {
        shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .detached
    }

    fn is_released(shared: &SyncMutex<FakeShared>) -> bool {
        shared
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .released
    }

    fn db_error(severity: DbSeverity, code: &str, message: &str) -> TxIoFailure {
        TxIoFailure::Database(DbAnswer {
            severity,
            code: Some(code.to_owned()),
            message: message.to_owned(),
        })
    }

    fn test_session(io: Box<dyn TxIo>, timer: Arc<dyn TxTimer>) -> SqlTxSession {
        SqlTxSession::with_io("pg", io, timer, ACQUIRE, GRACE, MAX_CONNECTIONS)
    }

    /// Polls once; returns true when the future stayed pending.
    fn poll_once_pending<F: Future>(pinned: &mut Pin<&mut F>) -> bool {
        let waker: Waker = noop_waker();
        let mut cx = Context::from_waker(&waker);
        matches!(pinned.as_mut().poll(&mut cx), Poll::Pending)
    }

    /// Timer whose deadlines have always already elapsed: proves the
    /// injected-clock seam is honored (the deadline arm wins the biased tie).
    struct AlwaysElapsed;

    impl TxTimer for AlwaysElapsed {
        fn now(&self) -> tokio::time::Instant {
            tokio::time::Instant::now()
        }

        fn sleep_until(
            &self,
            _deadline: tokio::time::Instant,
        ) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
            Box::pin(std::future::ready(()))
        }
    }

    /// 604c §A projection table, row for row.
    #[test]
    fn phase_projection_matches_604c_table() {
        let rows = [
            (TxPhase::Clean, TransactionState::Fresh),
            (TxPhase::BeginPending, TransactionState::Fresh),
            (TxPhase::Open, TransactionState::Active),
            (TxPhase::CommitInFlight, TransactionState::Finalizing),
            (TxPhase::RollbackInFlight, TransactionState::Finalizing),
            (TxPhase::Done, TransactionState::Done),
            (TxPhase::Detached, TransactionState::Detached),
        ];
        for (phase, expected) in rows {
            assert_eq!(
                project(phase),
                expected,
                "604c §A projection mismatch for {phase:?}"
            );
        }
    }

    /// §B.1: `begin` on an `Open` session is an idempotent no-op — the
    /// second call sends no SQL.
    #[tokio::test]
    async fn begin_is_idempotent_when_open() {
        let (io, shared) = FakeIo::channel();
        let session = test_session(io, Arc::new(TokioTimer));
        session.begin().await.expect("first begin acknowledges");
        assert_eq!(session.state(), TransactionState::Active);

        let second = session.begin().await;
        assert_eq!(second, Ok(()), "begin on Open is an idempotent no-op");
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN],
            "the idempotent call must not send BEGIN again"
        );
        assert_eq!(session.state(), TransactionState::Active);
    }

    /// §B.1: `begin` from a non-Clean, non-Open phase is `SessionAborted`
    /// with no SQL.
    #[tokio::test]
    async fn begin_from_non_clean_phases_is_aborted() {
        // Done entry (after an R2 clean rollback).
        let (io, shared) = FakeIo::channel();
        let session = test_session(io, Arc::new(TokioTimer));
        session.rollback().await.expect("clean rollback");
        let error = session.begin().await.expect_err("begin from Done aborts");
        assert_eq!(
            error,
            TransactionError::SessionAborted {
                datasource: "pg".to_owned()
            }
        );
        assert!(
            issued_of(&shared).is_empty(),
            "the aborted begin must send no SQL"
        );

        // BeginPending entry (a begin future dropped mid-BEGIN): the block
        // scope drops the pinned future and frees the slot.
        let (io, shared) = FakeIo::channel();
        FakeIo::script(SQL_BEGIN, Scripted::Pending, &shared);
        let session = test_session(io, Arc::new(TokioTimer));
        {
            let mut begin = std::pin::pin!(session.begin());
            assert!(poll_once_pending(&mut begin), "BEGIN must stay pending");
        }
        let error = session
            .begin()
            .await
            .expect_err("begin from BeginPending aborts");
        assert_eq!(
            error,
            TransactionError::SessionAborted {
                datasource: "pg".to_owned()
            }
        );
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN],
            "the aborted attempt must not send another BEGIN"
        );
    }

    /// §B.1 + Detached-is-terminal under a begin/detach race: a concurrent
    /// begin can never overwrite the terminal phase. Begin #1 stalls inside
    /// its BEGIN write (slot held); the racing begin #2 observes `Busy`
    /// without touching state (the phase is judged only under the slot
    /// lock); when begin #1's own deadline fires it detaches (§B.4) — begin
    /// #3 then sees `Detached` under the lock and reports `SessionAborted`,
    /// leaving the phase `Detached` (no `BeginPending` overwrite) and sending
    /// no SQL.
    #[tokio::test(start_paused = true)]
    async fn begin_racing_detach_never_overwrites_detached() {
        let (io, shared) = FakeIo::channel();
        FakeIo::script(SQL_BEGIN, Scripted::Pending, &shared);
        let session = test_session(io, Arc::new(TokioTimer));

        // Begin #1: holds the slot inside its BEGIN write.
        let mut first = std::pin::pin!(session.begin());
        assert!(poll_once_pending(&mut first), "the BEGIN write must stall");
        assert_eq!(issued_of(&shared), vec![SQL_BEGIN]);

        // Begin #2, racing while the slot is held: Busy, no state touched.
        let racing = session.begin().await;
        assert_eq!(
            racing,
            Err(TransactionError::Busy {
                datasource: "pg".to_owned()
            }),
            "the raced begin is rejected on the held slot before any phase read"
        );

        // Begin #1's own deadline fires → §B.4: detach, `PoolExhausted`.
        let first_outcome = first.await;
        assert_eq!(
            first_outcome,
            Err(TransactionError::PoolExhausted {
                datasource: "pg".to_owned(),
                max_connections: MAX_CONNECTIONS,
                waited_ms: ACQUIRE.as_millis() as u64,
            }),
            "the stalled begin detaches per §B.4"
        );
        assert_eq!(session.state(), TransactionState::Detached);

        // Begin #3 after the detach: `SessionAborted`, phase stays
        // `Detached`, no further SQL.
        let third = session.begin().await;
        assert_eq!(
            third,
            Err(TransactionError::SessionAborted {
                datasource: "pg".to_owned()
            }),
            "begin on a Detached session aborts, never overwrites the terminal phase"
        );
        assert_eq!(session.state(), TransactionState::Detached);
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN],
            "neither raced begin sent SQL"
        );
    }

    /// §B.4: BEGIN stalled past its own deadline → `PoolExhausted` naming
    /// datasource, max_connections and the waited time; detach; NO cleanup
    /// SQL. A further rollback is the R1 row (no SQL, `RollbackUncertain`).
    #[tokio::test(start_paused = true)]
    async fn begin_own_timeout_reports_pool_exhausted_detaches() {
        let (io, shared) = FakeIo::channel();
        FakeIo::script(SQL_BEGIN, Scripted::Pending, &shared);
        let session = test_session(io, Arc::new(TokioTimer));

        let error = session.begin().await.expect_err("begin deadline");
        assert_eq!(
            error,
            TransactionError::PoolExhausted {
                datasource: "pg".to_owned(),
                max_connections: MAX_CONNECTIONS,
                // Sane waited semantics: measures from the begin start, and
                // the virtual clock jumps exactly to the deadline.
                waited_ms: ACQUIRE.as_millis() as u64,
            }
        );
        assert!(
            ACQUIRE > Duration::ZERO,
            "the begin deadline must be a positive bound the wait is measured against"
        );
        assert_eq!(session.state(), TransactionState::Detached);
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN],
            "no cleanup SQL after the begin timeout"
        );
        assert!(is_detached(&shared), "the begin timeout detaches");
        assert!(
            !is_released(&shared),
            "a detached connection is never re-pooled"
        );

        // R1: a detached session never sends SQL again.
        let result = session.rollback().await;
        assert_eq!(
            result,
            Err(TransactionError::RollbackUncertain {
                datasource: "pg".to_owned()
            })
        );
        assert_eq!(issued_of(&shared), vec![SQL_BEGIN], "R1 sends no SQL");
    }

    /// §B.5: any driver error at BEGIN → detach + `RollbackUncertain`; NO
    /// cleanup SQL.
    #[tokio::test]
    async fn begin_driver_error_detaches_reports_rollback_uncertain() {
        let (io, shared) = FakeIo::channel();
        FakeIo::script(
            SQL_BEGIN,
            Scripted::Fail(db_error(DbSeverity::Other, "08006", "connection failure")),
            &shared,
        );
        let session = test_session(io, Arc::new(TokioTimer));

        let error = session.begin().await.expect_err("begin driver error");
        assert_eq!(
            error,
            TransactionError::RollbackUncertain {
                datasource: "pg".to_owned()
            }
        );
        assert_eq!(session.state(), TransactionState::Detached);
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN],
            "no cleanup SQL after a begin driver error"
        );
        assert!(is_detached(&shared));
    }

    /// The injected-clock seam: a timer whose deadlines have already elapsed
    /// must time the BEGIN out before any statement reaches the driver, even
    /// though the fake would acknowledge it.
    #[tokio::test]
    async fn injected_timer_times_out_begin() {
        let (io, shared) = FakeIo::channel();
        let session = test_session(io, Arc::new(AlwaysElapsed));

        let error = session.begin().await.expect_err("injected deadline");
        assert_eq!(
            error,
            TransactionError::PoolExhausted {
                datasource: "pg".to_owned(),
                max_connections: MAX_CONNECTIONS,
                waited_ms: 0,
            }
        );
        assert_eq!(session.state(), TransactionState::Detached);
        assert!(
            issued_of(&shared).is_empty(),
            "an already-elapsed deadline cancels the write before the driver sees it"
        );
        assert!(is_detached(&shared));
    }

    /// R7 classifier table: severity-`ERROR` SQLSTATE classes 23/40 are the
    /// ONLY affirmative rejections (`CommitFailed` + exactly one cleanup
    /// ROLLBACK); FATAL 57P01, class-57 57014, class-08 08006, and transport
    /// failures are `CommitUncertain` — no ROLLBACK, no rollback claim,
    /// detach.
    #[tokio::test]
    async fn transacted_commit_classifier_table() {
        let rows: Vec<(&str, TxIoFailure, TransactionError, bool)> = vec![
            (
                "23503 integrity violation",
                db_error(DbSeverity::Error, "23503", "foreign key violated"),
                TransactionError::CommitFailed {
                    datasource: "pg".to_owned(),
                    reason: "SQLSTATE 23503: foreign key violated".to_owned(),
                },
                true,
            ),
            (
                "40001 transaction abort class",
                db_error(DbSeverity::Error, "40001", "could not serialize access"),
                TransactionError::CommitFailed {
                    datasource: "pg".to_owned(),
                    reason: "SQLSTATE 40001: could not serialize access".to_owned(),
                },
                true,
            ),
            (
                "57P01 backend fatal",
                db_error(
                    DbSeverity::Other,
                    "57P01",
                    "terminating connection due to administrator command",
                ),
                TransactionError::CommitUncertain {
                    datasource: "pg".to_owned(),
                },
                false,
            ),
            (
                "57014 query canceled (class 57)",
                db_error(
                    DbSeverity::Error,
                    "57014",
                    "canceling statement due to user request",
                ),
                TransactionError::CommitUncertain {
                    datasource: "pg".to_owned(),
                },
                false,
            ),
            (
                "08006 connection failure (class 08)",
                db_error(DbSeverity::Error, "08006", "connection failure"),
                TransactionError::CommitUncertain {
                    datasource: "pg".to_owned(),
                },
                false,
            ),
            (
                "io transport error",
                TxIoFailure::Lost,
                TransactionError::CommitUncertain {
                    datasource: "pg".to_owned(),
                },
                false,
            ),
        ];

        for (name, commit_failure, expected, cleanup_issued) in rows {
            let (io, shared) = FakeIo::channel();
            FakeIo::script(SQL_COMMIT, Scripted::Fail(commit_failure), &shared);
            let session = test_session(io, Arc::new(TokioTimer));
            session.begin().await.expect("BEGIN must acknowledge");
            assert_eq!(session.state(), TransactionState::Active, "row {name}");

            let result = session.commit().await;
            assert_eq!(result, Err(expected), "row {name}");

            let expected_issued = if cleanup_issued {
                vec![SQL_BEGIN, SQL_COMMIT, SQL_ROLLBACK]
            } else {
                vec![SQL_BEGIN, SQL_COMMIT]
            };
            assert_eq!(issued_of(&shared), expected_issued, "row {name}");
            if cleanup_issued {
                assert!(
                    is_released(&shared),
                    "cleanup acknowledged: connection returned to the pool (row {name})"
                );
                assert!(
                    !is_detached(&shared),
                    "no detach after an acknowledged cleanup (row {name})"
                );
                assert_eq!(session.state(), TransactionState::Done, "row {name}");
            } else {
                assert!(is_detached(&shared), "detach immediately (row {name})");
                assert!(
                    !is_released(&shared),
                    "no rollback claim and no re-pool (row {name})"
                );
                assert_eq!(session.state(), TransactionState::Detached, "row {name}");
            }
        }
    }

    /// Port contract: `commit` requires `Active`; otherwise no SQL.
    #[tokio::test]
    async fn commit_requires_open_sends_no_sql() {
        // Clean entry.
        let (io, shared) = FakeIo::channel();
        let session = test_session(io, Arc::new(TokioTimer));
        let error = session
            .commit()
            .await
            .expect_err("commit from Clean aborts");
        assert_eq!(
            error,
            TransactionError::SessionAborted {
                datasource: "pg".to_owned()
            }
        );
        assert!(
            issued_of(&shared).is_empty(),
            "the aborted commit must send no SQL"
        );

        // Done entry (after an acknowledged rollback).
        let (io, shared) = FakeIo::channel();
        let session = test_session(io, Arc::new(TokioTimer));
        session.begin().await.expect("BEGIN must acknowledge");
        session.rollback().await.expect("rollback acknowledges");
        let error = session.commit().await.expect_err("commit from Done aborts");
        assert_eq!(
            error,
            TransactionError::SessionAborted {
                datasource: "pg".to_owned()
            }
        );
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN, SQL_ROLLBACK],
            "the aborted commit must send no SQL"
        );
    }

    /// 604d defensive rows: a `Finalizing`/`Done` rollback entry sends no
    /// SQL, logs `error!`, changes no phase, and claims NO detachment.
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn defensive_finalization_entry_logs_error_without_detach_claim() {
        // CommitInFlight entry: a commit future dropped mid-COMMIT.
        let (io, shared) = FakeIo::channel();
        FakeIo::script(SQL_COMMIT, Scripted::Pending, &shared);
        let session = test_session(io, Arc::new(TokioTimer));
        session.begin().await.expect("BEGIN must acknowledge");
        {
            let mut commit = std::pin::pin!(session.commit());
            assert!(poll_once_pending(&mut commit), "COMMIT must stay pending");
            assert_eq!(session.state(), TransactionState::Finalizing);
        }

        let result = session.rollback().await;
        assert_eq!(
            result,
            Err(TransactionError::RollbackUncertain {
                datasource: "pg".to_owned()
            }),
            "defensive Finalizing entry returns Err, never an outcome claim"
        );
        assert_eq!(
            session.state(),
            TransactionState::Finalizing,
            "the defensive entry changes no phase"
        );
        assert!(logs_contain("defensive finalization entry"));
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN, SQL_COMMIT],
            "no SQL on the defensive path"
        );
        assert!(
            !is_detached(&shared),
            "the defensive entry must NOT claim detachment happened"
        );
        assert!(!is_released(&shared));

        // Done entry: a further rollback after an acknowledged rollback.
        let (io, shared) = FakeIo::channel();
        let session = test_session(io, Arc::new(TokioTimer));
        session.begin().await.expect("BEGIN must acknowledge");
        assert_eq!(session.rollback().await, Ok(TransactionOutcome::RolledBack));
        let result = session.rollback().await;
        assert_eq!(
            result,
            Err(TransactionError::RollbackUncertain {
                datasource: "pg".to_owned()
            }),
            "defensive Done entry returns Err, never an outcome claim"
        );
        assert_eq!(
            session.state(),
            TransactionState::Done,
            "the defensive entry changes no phase"
        );
        assert!(logs_contain("defensive finalization entry"));
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN, SQL_ROLLBACK],
            "no SQL on the defensive path"
        );
        assert!(
            !is_detached(&shared),
            "the Done entry adds no detach: the connection was already released by the first rollback"
        );
    }

    /// R2: rollback from `Clean` — no SQL, guard disarmed, connection
    /// returned clean, `Ok(RolledBack)`.
    #[tokio::test]
    async fn r2_clean_rollback_returns_connection_clean() {
        let (io, shared) = FakeIo::channel();
        let session = test_session(io, Arc::new(TokioTimer));
        assert_eq!(session.state(), TransactionState::Fresh);
        assert!(!session.is_rollback_only());

        let result = session.rollback().await;
        assert_eq!(result, Ok(TransactionOutcome::RolledBack));
        assert_eq!(session.state(), TransactionState::Done);
        assert!(
            issued_of(&shared).is_empty(),
            "R2 sends no SQL: nothing was begun"
        );
        assert!(
            is_released(&shared),
            "the connection is returned to the pool clean"
        );
        assert!(!is_detached(&shared));
    }

    /// The rollback-only flag round-trips on the port's atomic and does not
    /// change session-level rollback mechanics (the segment owns the R4
    /// precedence per 604d).
    #[tokio::test]
    async fn rollback_only_flag_and_open_rollback_acks() {
        let (io, shared) = FakeIo::channel();
        let session = test_session(io, Arc::new(TokioTimer));
        assert!(!session.is_rollback_only());
        session.set_rollback_only();
        assert!(session.is_rollback_only());

        session.begin().await.expect("BEGIN must acknowledge");
        let result = session.rollback().await;
        assert_eq!(result, Ok(TransactionOutcome::RolledBack));
        assert_eq!(issued_of(&shared), vec![SQL_BEGIN, SQL_ROLLBACK]);
        assert!(
            session.is_rollback_only(),
            "finalization does not clear the flag"
        );
    }

    /// Busy admission (req "Busy block session"): a held slot fails any
    /// statement run with the typed `Busy` error, deterministically.
    #[tokio::test]
    async fn busy_slot_try_lock_reports_busy() {
        let (io, shared) = FakeIo::channel();
        let session = test_session(io, Arc::new(TokioTimer));

        let held = session
            .slot
            .try_lock()
            .expect("the slot must be free before the fixture holds it");
        let error = session.begin().await.expect_err("held slot");
        assert_eq!(
            error,
            TransactionError::Busy {
                datasource: "pg".to_owned()
            }
        );
        assert!(
            issued_of(&shared).is_empty(),
            "a busy-rejected begin sends no SQL"
        );
        drop(held);

        session.begin().await.expect("begin after the slot frees");
        assert_eq!(issued_of(&shared), vec![SQL_BEGIN]);
    }

    /// Frozen fixture `transacted_hygiene_unacknowledged` (spec admission
    /// clause): an unacknowledged hygiene ROLLBACK detaches the connection
    /// without sending BEGIN, and exactly one ROLLBACK attempt is made.
    #[tokio::test]
    async fn transacted_hygiene_unacknowledged() {
        let (mut io, shared) = FakeIo::channel();
        FakeIo::script(
            SQL_ROLLBACK,
            Scripted::Fail(db_error(DbSeverity::Error, "08006", "connection failure")),
            &shared,
        );
        let timer: Arc<dyn TxTimer> = Arc::new(TokioTimer);
        let now = timer.now();

        let error = admission_hygiene(
            &mut *io,
            Arc::clone(&timer),
            "pg",
            MAX_CONNECTIONS,
            now + Duration::from_secs(60),
            now + ACQUIRE,
        )
        .await
        .expect_err("unacknowledged hygiene");
        assert_eq!(
            error,
            TransactionError::RollbackUncertain {
                datasource: "pg".to_owned()
            }
        );
        assert_eq!(
            issued_of(&shared),
            vec![SQL_ROLLBACK],
            "exactly one ROLLBACK attempt, BEGIN never sent"
        );
        assert!(is_detached(&shared), "unacknowledged hygiene detaches");
    }

    /// Real-backend smoke over an in-memory SQLite pool: hygiene is skipped
    /// (SQLite errors on a clean-connection ROLLBACK; hygiene is PG-runtime
    /// territory exercised by the PG suite), but BEGIN/COMMIT/ROLLBACK run
    /// as plain statements through [`AnyConnIo`], and a released connection
    /// serves the pool again.
    #[tokio::test]
    async fn sqlite_pool_begin_commit_and_rollback_smoke() {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new() // allow-test-wait: in-memory sqlite connect, no network
            .max_connections(2)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");
        let timer: Arc<dyn TxTimer> = Arc::new(TokioTimer);

        let conn = pool.acquire().await.expect("first connection"); // allow-test-wait: pool max 2, no other holders
        let session = SqlTxSession::with_io(
            "pg",
            Box::new(AnyConnIo { conn: Some(conn) }),
            Arc::clone(&timer),
            ACQUIRE,
            GRACE,
            2,
        );
        session.begin().await.expect("real BEGIN");
        assert_eq!(session.state(), TransactionState::Active);
        let outcome = session.commit().await.expect("real COMMIT");
        assert_eq!(outcome, TransactionOutcome::Committed);
        assert_eq!(session.state(), TransactionState::Done);

        let conn = pool.acquire().await.expect("second connection"); // allow-test-wait: first session released, pool max 2
        let session = SqlTxSession::with_io(
            "pg",
            Box::new(AnyConnIo { conn: Some(conn) }),
            timer,
            ACQUIRE,
            GRACE,
            2,
        );
        session.begin().await.expect("real BEGIN");
        let outcome = session.rollback().await.expect("real ROLLBACK");
        assert_eq!(outcome, TransactionOutcome::RolledBack);
        assert_eq!(session.state(), TransactionState::Done);

        // Released connections serve the pool again.
        sqlx::query("SELECT 1")
            .execute(&pool)
            .await
            .expect("pool usable after finalization");
        pool.close().await;
    }

    /// R0: dropping the session mid-transaction detaches synchronously —
    /// no SQL, never re-pooled.
    #[tokio::test]
    async fn forced_drop_detaches_without_sql() {
        assert_forced_drop_from_open().await;
    }

    /// Frozen fixture `transacted_forced_drop_inline` (spec cancellation
    /// cutoff): the inline-dispatcher route-level drop lands with the
    /// executor wiring (step 9 / family); the armed `DiscardOnDrop` guard
    /// THIS fixture pins is the same mechanism that drop path triggers — the
    /// session is dropped with the transaction open, no SQL is sent, and the
    /// connection is never re-pooled.
    #[tokio::test]
    async fn transacted_forced_drop_inline() {
        assert_forced_drop_from_open().await;
    }

    async fn assert_forced_drop_from_open() {
        let (io, shared) = FakeIo::channel();
        let session = test_session(io, Arc::new(TokioTimer));
        session.begin().await.expect("BEGIN must acknowledge");
        assert_eq!(session.state(), TransactionState::Active);

        drop(session);
        assert_eq!(issued_of(&shared), vec![SQL_BEGIN], "R0 sends no SQL");
        assert!(is_detached(&shared), "R0 detaches synchronously");
        assert!(!is_released(&shared), "R0 never returns the connection");
    }

    /// R3 via the total rollback: a begin future left pending in
    /// `BeginPending` (BEGIN write never acknowledged) — `rollback()`
    /// detaches with no further SQL and reports `RollbackUncertain`.
    #[tokio::test]
    async fn transacted_begin_pending_deadline() {
        let (io, shared) = FakeIo::channel();
        FakeIo::script(SQL_BEGIN, Scripted::Pending, &shared);
        let session = test_session(io, Arc::new(TokioTimer));

        // Leave BEGIN pending: one poll (BeginPending is set before the
        // write), then drop the future — the phase stays BeginPending and
        // the slot frees.
        {
            let mut begin = std::pin::pin!(session.begin());
            assert!(poll_once_pending(&mut begin), "BEGIN must stay pending");
            assert_eq!(
                session.state(),
                TransactionState::Fresh,
                "BeginPending projects lossily onto Fresh"
            );
        }

        let result = session.rollback().await;
        assert_eq!(
            result,
            Err(TransactionError::RollbackUncertain {
                datasource: "pg".to_owned()
            }),
            "R3: protocol position unknown, nothing acknowledged"
        );
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN],
            "R3 sends no SQL: no ROLLBACK on an unacknowledged BEGIN"
        );
        assert!(is_detached(&shared), "R3 detaches the connection");
        assert!(
            !is_released(&shared),
            "a detached connection is never returned to the pool"
        );
        assert_eq!(session.state(), TransactionState::Detached);
    }

    /// Finalize grace, COMMIT leg — frozen fixture `transacted_stalled_commit`
    /// (spec "Block timeout"): a COMMIT stalled past `finalize_start + 5 s` is
    /// `CommitUncertain` — detach, no cleanup ROLLBACK, no claim.
    #[tokio::test(start_paused = true)]
    async fn transacted_stalled_commit() {
        let (io, shared) = FakeIo::channel();
        FakeIo::script(SQL_COMMIT, Scripted::Pending, &shared);
        let session = test_session(io, Arc::new(TokioTimer));
        session.begin().await.expect("BEGIN must acknowledge");

        let result = session.commit().await;
        assert_eq!(
            result,
            Err(TransactionError::CommitUncertain {
                datasource: "pg".to_owned()
            }),
            "grace elapsed: the commit outcome is uncertain"
        );
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN, SQL_COMMIT],
            "no cleanup ROLLBACK after a stalled COMMIT"
        );
        assert!(is_detached(&shared), "the grace expiry detaches");
        assert!(
            !is_released(&shared),
            "a detached connection is never re-pooled"
        );
        assert_eq!(session.state(), TransactionState::Detached);
    }

    /// Finalize grace, ROLLBACK leg — frozen fixture
    /// `transacted_stalled_rollback` (spec "Block timeout"): a ROLLBACK
    /// stalled past the grace is `RollbackUncertain` — detach, no further SQL.
    #[tokio::test(start_paused = true)]
    async fn transacted_stalled_rollback() {
        let (io, shared) = FakeIo::channel();
        FakeIo::script(SQL_ROLLBACK, Scripted::Pending, &shared);
        let session = test_session(io, Arc::new(TokioTimer));
        session.begin().await.expect("BEGIN must acknowledge");

        let result = session.rollback().await;
        assert_eq!(
            result,
            Err(TransactionError::RollbackUncertain {
                datasource: "pg".to_owned()
            }),
            "grace elapsed: the rollback is not acknowledged"
        );
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN, SQL_ROLLBACK],
            "exactly one ROLLBACK attempt within the grace"
        );
        assert!(is_detached(&shared), "the grace expiry detaches");
        assert!(
            !is_released(&shared),
            "a detached connection is never re-pooled"
        );
        assert_eq!(session.state(), TransactionState::Detached);
    }

    /// Frozen "CommitFailed stands" (spec Cancellation cutoff, precedence
    /// over 604c R7's sub-row): COMMIT rejected with 23503 but the cleanup
    /// ROLLBACK itself stalls past the grace — the typed `CommitFailed` is
    /// the primary in BOTH cleanup outcomes; the cleanup uncertainty is
    /// observable through the warn! log and the `Detached` state only. The
    /// connection detaches, never re-pools. Adjudication:
    /// `openspec/changes/sqltrans-exec/design.md` "Edge adjudications".
    #[tracing_test::traced_test]
    #[tokio::test(start_paused = true)]
    async fn commit_cleanup_unacknowledged_keeps_commit_failed() {
        let (io, shared) = FakeIo::channel();
        FakeIo::script(
            SQL_COMMIT,
            Scripted::Fail(db_error(DbSeverity::Error, "23503", "foreign key violated")),
            &shared,
        );
        FakeIo::script(SQL_ROLLBACK, Scripted::Pending, &shared);
        let session = test_session(io, Arc::new(TokioTimer));
        session.begin().await.expect("BEGIN must acknowledge");

        let result = session.commit().await;
        assert_eq!(
            result,
            Err(TransactionError::CommitFailed {
                datasource: "pg".to_owned(),
                reason: "SQLSTATE 23503: foreign key violated".to_owned(),
            }),
            "the typed commit failure stands; cleanup uncertainty never replaces it"
        );
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN, SQL_COMMIT, SQL_ROLLBACK],
            "exactly one cleanup ROLLBACK attempt within the remaining grace"
        );
        assert!(is_detached(&shared), "unacknowledged cleanup detaches");
        assert!(
            !is_released(&shared),
            "the connection is never returned to the pool with unknown state"
        );
        assert_eq!(session.state(), TransactionState::Detached);
        assert!(
            logs_contain("not acknowledged"),
            "the cleanup uncertainty is observable through the warn! log"
        );
    }

    /// Hygiene deadline (MINOR-1 semantics): the `Timeout` error measures the
    /// time elapsed from the hygiene start, not from the deadline.
    #[tokio::test(start_paused = true)]
    async fn hygiene_deadline_reports_timeout_elapsed_from_start() {
        let (mut io, shared) = FakeIo::channel();
        FakeIo::script(SQL_ROLLBACK, Scripted::Pending, &shared);
        let timer: Arc<dyn TxTimer> = Arc::new(TokioTimer);
        let now = timer.now();
        let block_deadline = Duration::from_millis(10);

        let error = admission_hygiene(
            &mut *io,
            Arc::clone(&timer),
            "pg",
            MAX_CONNECTIONS,
            now + block_deadline,
            now + Duration::from_secs(60),
        )
        .await
        .expect_err("block deadline elapses while hygiene is pending");
        assert_eq!(
            error,
            TransactionError::Timeout {
                datasource: "pg".to_owned(),
                elapsed_ms: block_deadline.as_millis() as u64,
            },
            "elapsed must measure from the hygiene start (> 0), not from the deadline"
        );
        assert_eq!(
            issued_of(&shared),
            vec![SQL_ROLLBACK],
            "exactly one ROLLBACK attempt, BEGIN never sent"
        );
        assert!(
            is_detached(&shared),
            "deadline expiry during hygiene detaches"
        );
    }

    /// One admission-hygiene deadline case with the hygiene ROLLBACK held
    /// pending: returns the typed error after asserting the invariant shared
    /// by every deadline ordering — exactly one ROLLBACK attempt, BEGIN never
    /// sent, the connection detached and never released.
    async fn hygiene_deadline_case(block: Duration, acquire: Duration) -> TransactionError {
        let (mut io, shared) = FakeIo::channel();
        FakeIo::script(SQL_ROLLBACK, Scripted::Pending, &shared);
        let timer: Arc<dyn TxTimer> = Arc::new(TokioTimer);
        let now = timer.now();

        let error = admission_hygiene(
            &mut *io,
            Arc::clone(&timer),
            "pg",
            MAX_CONNECTIONS,
            now + block,
            now + acquire,
        )
        .await
        .expect_err("hygiene stays pending past both deadlines");
        assert_eq!(
            issued_of(&shared),
            vec![SQL_ROLLBACK],
            "exactly one ROLLBACK attempt, BEGIN never sent"
        );
        assert!(
            is_detached(&shared),
            "deadline expiry during hygiene detaches"
        );
        assert!(
            !is_released(&shared),
            "a detached connection is never re-pooled"
        );
        error
    }

    /// Frozen fixture `transacted_hygiene_deadline_expiry` (spec admission
    /// clause): the hygiene ROLLBACK is left pending while the virtual clock
    /// advances through the three deadline orderings — `PoolExhausted` for
    /// acquire-first, `Timeout` for block-first, and `Timeout` for the tie
    /// (the biased block arm) — with exactly one ROLLBACK attempt, BEGIN never
    /// sent, and the connection detached.
    #[tokio::test(start_paused = true)]
    async fn transacted_hygiene_deadline_expiry() {
        let cases: [(&str, Duration, Duration, TransactionError); 3] = [
            (
                "acquire-first: acquire 10 ms < block 60 s",
                Duration::from_secs(60),
                Duration::from_millis(10),
                TransactionError::PoolExhausted {
                    datasource: "pg".to_owned(),
                    max_connections: MAX_CONNECTIONS,
                    waited_ms: 10,
                },
            ),
            (
                "block-first: block 10 ms < acquire 60 s",
                Duration::from_millis(10),
                Duration::from_secs(60),
                TransactionError::Timeout {
                    datasource: "pg".to_owned(),
                    elapsed_ms: 10,
                },
            ),
            (
                "tie: both 10 ms — the biased block arm wins",
                Duration::from_millis(10),
                Duration::from_millis(10),
                TransactionError::Timeout {
                    datasource: "pg".to_owned(),
                    elapsed_ms: 10,
                },
            ),
        ];

        for (label, block, acquire, expected) in cases {
            let error = hygiene_deadline_case(block, acquire).await;
            assert_eq!(error, expected, "{label}");
        }
    }

    /// R0 arms AT checkout (frozen spec: "The discard guard is armed on
    /// connection checkout"): dropping the open future while the admission
    /// hygiene `ROLLBACK` is still pending detaches the checked-out
    /// connection — it is never released back to the pool. This is the
    /// checkout-guard twin of `transacted_forced_drop_inline`: the
    /// guard-bearing session does not exist yet when the open future dies,
    /// so the arming cannot wait for `with_io`.
    #[tokio::test(start_paused = true)]
    async fn open_future_dropped_mid_hygiene_detaches_at_checkout() {
        let (io, shared) = FakeIo::channel();
        FakeIo::script(SQL_ROLLBACK, Scripted::Pending, &shared);
        let timer: Arc<dyn TxTimer> = Arc::new(TokioTimer);
        let now = timer.now();

        // One poll: the hygiene `ROLLBACK` is issued and stays pending;
        // leaving the block drops the open future mid-hygiene.
        {
            let mut open = std::pin::pin!(SqlTxSession::admit(
                "pg".into(),
                CheckedOutBackend::arm(io),
                now + Duration::from_secs(60),
                now + Duration::from_secs(60),
                ACQUIRE,
                MAX_CONNECTIONS,
                Arc::clone(&timer),
            ));
            assert!(poll_once_pending(&mut open), "hygiene must stay pending");
            assert_eq!(
                issued_of(&shared),
                vec![SQL_ROLLBACK],
                "exactly one ROLLBACK attempt, BEGIN never sent"
            );
        }

        assert!(
            is_detached(&shared),
            "a drop mid-hygiene detaches (R0 arms at checkout)"
        );
        assert!(
            !is_released(&shared),
            "the detached connection is never re-pooled"
        );
    }

    /// Frozen fixture `transacted_acquire_deadline_first` (spec "Block
    /// timeout"): the acquire deadline elapses first → typed `PoolExhausted`
    /// naming the datasource, the pool size, and the waited milliseconds.
    #[tokio::test(start_paused = true)]
    async fn transacted_acquire_deadline_first() {
        let error = hygiene_deadline_case(Duration::from_secs(60), Duration::from_millis(10)).await;
        match error {
            TransactionError::PoolExhausted {
                datasource,
                max_connections,
                waited_ms,
            } => {
                assert_eq!(datasource, "pg", "PoolExhausted names the datasource");
                assert_eq!(max_connections, MAX_CONNECTIONS);
                assert_eq!(waited_ms, 10, "waited_ms measures from the hygiene start");
            }
            other => panic!("expected typed PoolExhausted, got {other:?}"),
        }
    }

    /// Frozen fixture `transacted_block_deadline_first` (spec "Block
    /// timeout"): with block deadline 30 s and acquire deadline 60 s, the
    /// typed `Timeout` names the datasource and the elapsed time measured
    /// from block entry.
    #[tokio::test(start_paused = true)]
    async fn transacted_block_deadline_first() {
        let error = hygiene_deadline_case(Duration::from_secs(30), Duration::from_secs(60)).await;
        assert_eq!(
            error,
            TransactionError::Timeout {
                datasource: "pg".to_owned(),
                elapsed_ms: 30_000,
            },
            "elapsed is measured from block entry, not from the deadline"
        );
    }

    /// Frozen fixture `transacted_deadline_tie` (spec "Block timeout"): both
    /// deadlines equal (30 s) at admission → the biased block arm reports
    /// `Timeout`, never `PoolExhausted`. Distinct from the body-race tie
    /// fixture `transacted_deadline_tie_selects_timeout` in camel-core.
    #[tokio::test(start_paused = true)]
    async fn transacted_deadline_tie() {
        let error = hygiene_deadline_case(Duration::from_secs(30), Duration::from_secs(30)).await;
        assert_eq!(
            error,
            TransactionError::Timeout {
                datasource: "pg".to_owned(),
                elapsed_ms: 30_000,
            },
            "the tie resolves to the block-deadline error, never PoolExhausted"
        );
    }

    /// Frozen "Block timeout" instant comparison, both deadlines already
    /// elapsed at the FIRST poll (paused clock): the race is decided by the
    /// stored absolute instants, not by the biased polling order. With the
    /// acquire deadline strictly earlier (`acquire_abs < block_abs`), the
    /// admission reports `PoolExhausted` even though the block timer arm is
    /// polled first; the tie (equal instants) reports `Timeout`.
    ///
    /// Distinct from [`transacted_hygiene_deadline_expiry`], whose orderings
    /// fire while both timers still run — this fixture is the same-poll
    /// pathology: at the first poll both `sleep_until` futures are ready.
    #[tokio::test(start_paused = true)]
    async fn transacted_deadline_both_elapsed_at_first_poll() {
        async fn case(block: Duration, acquire: Duration) -> TransactionError {
            let (mut io, shared) = FakeIo::channel();
            FakeIo::script(SQL_ROLLBACK, Scripted::Pending, &shared);
            let timer: Arc<dyn TxTimer> = Arc::new(TokioTimer);
            let now = timer.now();
            let hygiene = std::pin::pin!(admission_hygiene(
                &mut *io,
                Arc::clone(&timer),
                "pg",
                MAX_CONNECTIONS,
                now + block,
                now + acquire,
            ));
            // Advance past BOTH deadlines before the first poll: every race
            // arm is ready at once, so only the instant comparison decides.
            tokio::time::sleep(block.max(acquire) + Duration::from_secs(1)).await;
            let error = hygiene.await.expect_err("both deadlines are spent");
            assert!(
                issued_of(&shared).is_empty(),
                "the race resolved before any I/O was polled"
            );
            assert!(is_detached(&shared), "the losing deadline detaches");
            error
        }

        let error = case(Duration::from_secs(60), Duration::from_millis(10)).await;
        match error {
            TransactionError::PoolExhausted {
                datasource,
                max_connections,
                ..
            } => {
                assert_eq!(datasource, "pg", "PoolExhausted names the datasource");
                assert_eq!(max_connections, MAX_CONNECTIONS);
            }
            other => panic!(
                "acquire_abs < block_abs with both elapsed must report PoolExhausted, got {other:?}"
            ),
        }

        let error = case(Duration::from_millis(10), Duration::from_millis(10)).await;
        assert!(
            matches!(error, TransactionError::Timeout { .. }),
            "equal instants report Timeout on the tie, got {error:?}"
        );
    }

    /// Frozen "Block timeout" instant comparison at the OPEN race site
    /// (checkout vs the two deadlines): the injected timer's sleeps are
    /// always ready, so both deadline arms resolve at the first poll —
    /// exactly the pathology the biased order cannot decide. With the
    /// acquire deadline strictly earlier, the open reports `PoolExhausted`;
    /// with equal instants, `Timeout`. The pool is never polled (`acquire`
    /// loses the race) — an in-memory SQLite pool stands in for the real one.
    #[tokio::test]
    async fn transacted_open_race_both_deadlines_elapsed_at_first_poll() {
        sqlx::any::install_default_drivers();
        let pool = sqlx::any::AnyPoolOptions::new() // allow-test-wait: in-memory sqlite connect, no network
            .max_connections(2)
            .connect("sqlite::memory:")
            .await
            .expect("in-memory sqlite pool");
        let timer: Arc<dyn TxTimer> = Arc::new(AlwaysElapsed);
        let now = timer.now();

        // acquire_abs = now + 10 ms is strictly before block_abs = now + 60 s
        // and both sleeps resolve at the first poll → PoolExhausted.
        let open = SqlTxSession::open(
            "pg",
            &pool,
            now + Duration::from_secs(60),
            Duration::from_millis(10),
            Arc::clone(&timer),
        )
        .await;
        let error = match open {
            Err(error) => error,
            Ok(_) => panic!("both admission deadlines are spent"),
        };
        match error {
            TransactionError::PoolExhausted { datasource, .. } => {
                assert_eq!(datasource, "pg", "PoolExhausted names the datasource");
            }
            other => panic!(
                "acquire_abs < block_abs with both elapsed must report PoolExhausted, got {other:?}"
            ),
        }

        // Equal instants → the tie reports Timeout, never PoolExhausted.
        let open = SqlTxSession::open(
            "pg",
            &pool,
            now + Duration::from_millis(10),
            Duration::from_millis(10),
            timer,
        )
        .await;
        let error = match open {
            Err(error) => error,
            Ok(_) => panic!("both admission deadlines are spent"),
        };
        assert!(
            matches!(error, TransactionError::Timeout { .. }),
            "equal instants report Timeout on the tie, got {error:?}"
        );
    }

    /// R0 from `Clean`: dropping the session before any statement detaches
    /// synchronously, no SQL, never re-pooled.
    #[tokio::test]
    async fn forced_drop_from_clean_detaches_without_sql() {
        let (io, shared) = FakeIo::channel();
        let session = test_session(io, Arc::new(TokioTimer));
        assert_eq!(session.state(), TransactionState::Fresh);

        drop(session);
        assert!(issued_of(&shared).is_empty(), "R0 from Clean sends no SQL");
        assert!(is_detached(&shared), "R0 detaches synchronously");
        assert!(!is_released(&shared), "R0 never returns the connection");
    }

    /// R0 from `BeginPending`: a begin future left pending, then the session
    /// dropped — synchronous detach, no further SQL.
    #[tokio::test]
    async fn forced_drop_from_begin_pending_detaches_without_sql() {
        let (io, shared) = FakeIo::channel();
        FakeIo::script(SQL_BEGIN, Scripted::Pending, &shared);
        let session = test_session(io, Arc::new(TokioTimer));
        {
            let mut begin = std::pin::pin!(session.begin());
            assert!(poll_once_pending(&mut begin), "BEGIN must stay pending");
        }

        drop(session);
        assert_eq!(
            issued_of(&shared),
            vec![SQL_BEGIN],
            "R0 from BeginPending sends no further SQL"
        );
        assert!(is_detached(&shared), "R0 detaches synchronously");
        assert!(!is_released(&shared), "R0 never returns the connection");
    }
}
