//! Provider-neutral transaction port for `transacted { datasource }` blocks.
//!
//! This module is the `camel-api` half of the sql-transactions capability: it
//! defines the object-safe [`TransactionManager`] / [`TransactionSession`]
//! surface, the [`TransactionState`] lifecycle machine, the
//! [`TransactionOutcome`] finalization result, and the typed
//! [`TransactionError`] family. The port is implemented by the sql component
//! (slice 2) and resolved by datasource name from a `camel-core` registry;
//! `camel-api` carries no driver or SQL value model.
//!
//! The surface is deliberately final for slice 2: lazy begin, the rollback-only
//! flag, finalization outcomes, and `#[non_exhaustive]` error/outcome types are
//! all present here, so adding execution and registry wiring needs no signature
//! change. Sessions are `'static` trait objects so the implementing crate can
//! downcast to its concrete session for statement execution.
//!
//! The [`join`] submodule carries the runtime join channels (a `task_local!`
//! block-session cell and the baked join scope) that bind a joined sql step to
//! the block session it must run inside.

pub mod join;

use std::any::Any;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use thiserror::Error;

/// Lifecycle state of a [`TransactionSession`].
///
/// The session tracks the state explicitly rather than inferring it from the
/// borrow of a guard: `Fresh → Active → Finalizing → Done | Detached`.
/// `Fresh` means no `BEGIN` has been sent yet (begin is lazy, per the
/// sql-transactions requirement "Lazy begin and block-owned commit/rollback"),
/// `Finalizing` covers an in-flight `COMMIT`/`ROLLBACK`, and `Detached` records
/// a connection that was discarded instead of returned to the pool (requirement
/// "Uncertain finalization detaches the connection").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransactionState {
    /// No statement has been sent on the session yet; `BEGIN` is lazy.
    Fresh,
    /// `BEGIN` was acknowledged and the session accepts joined work.
    Active,
    /// A `COMMIT` or `ROLLBACK` is in flight; no new joined work is admitted.
    Finalizing,
    /// Finalization completed and the session holds no transaction.
    Done,
    /// The backing connection was discarded; the session is unusable.
    Detached,
}

/// Result of finalizing a [`TransactionSession`].
///
/// Returned by [`TransactionSession::commit`] and
/// [`TransactionSession::rollback`]; `#[non_exhaustive]` so slice 2 can add
/// outcomes without breaking implementors. [`TransactionOutcome::Detached`] is
/// the observable counterpart of a connection that was dropped rather than
/// returned to the pool (requirement "Uncertain finalization detaches the
/// connection").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransactionOutcome {
    /// `COMMIT` was acknowledged by the database.
    Committed,
    /// `ROLLBACK` was acknowledged by the database.
    RolledBack,
    /// Finalization could not be acknowledged and the connection was discarded.
    Detached,
}

/// Typed transaction failures for load-time validation and runtime finalization.
///
/// Every variant names the offender (step or statement) and, where one exists,
/// the datasource, so operators can discriminate programmatically. The variants
/// map one-to-one onto the sql-transactions requirements named in each doc
/// comment. `#[non_exhaustive]` lets slice 2 grow the family without a breaking
/// change.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum TransactionError {
    /// No [`TransactionManager`] is registered for the block datasource.
    ///
    /// Serves the requirement "Load-time validation is fail-closed and typed"
    /// (unknown datasource with no registered TransactionManager) and the
    /// requirement "TransactionManager port and registry resolution"
    /// (fail closed, named error).
    #[error("no TransactionManager for datasource '{datasource}'")]
    ManagerMissing {
        /// Datasource named by the transacted block.
        datasource: String,
    },

    /// A parallel split reaches a joined sql step inside the block.
    ///
    /// Serves the requirement "Load-time validation is fail-closed and typed".
    #[error(
        "parallel split step '{step}' inside a transacted block on datasource '{datasource}' \
         cannot join the transaction"
    )]
    ParallelInBlock {
        /// The split step that escapes the transaction scope.
        step: String,
        /// Datasource of the enclosing transacted block.
        datasource: String,
    },

    /// A multicast reaches a joined sql step inside the block.
    ///
    /// Serves the requirement "Load-time validation is fail-closed and typed".
    #[error(
        "multicast step '{step}' inside a transacted block on datasource '{datasource}' \
         cannot join the transaction"
    )]
    MulticastInBlock {
        /// The multicast step that escapes the transaction scope.
        step: String,
        /// Datasource of the enclosing transacted block.
        datasource: String,
    },

    /// A wire tap reaches a joined sql step inside the block.
    ///
    /// Serves the requirement "Load-time validation is fail-closed and typed".
    #[error(
        "wireTap step '{step}' inside a transacted block on datasource '{datasource}' \
         cannot join the transaction"
    )]
    WireTapInBlock {
        /// The wire-tap step that escapes the transaction scope.
        step: String,
        /// Datasource of the enclosing transacted block.
        datasource: String,
    },

    /// An asynchronous producer handoff reaches a joined sql step inside the block.
    ///
    /// Serves the requirement "Load-time validation is fail-closed and typed".
    #[error(
        "async handoff at step '{step}' inside a transacted block on datasource '{datasource}' \
         cannot join the transaction"
    )]
    AsyncInBlock {
        /// The step whose producer scheme hands the exchange to another task.
        step: String,
        /// Datasource of the enclosing transacted block.
        datasource: String,
    },

    /// A `seda:` handoff reaches a joined sql step inside the block.
    ///
    /// Serves the requirement "Load-time validation is fail-closed and typed".
    #[error(
        "seda handoff at step '{step}' inside a transacted block on datasource '{datasource}' \
         cannot join the transaction"
    )]
    SedaInBlock {
        /// The step that hands the exchange to a seda queue.
        step: String,
        /// Datasource of the enclosing transacted block.
        datasource: String,
    },

    /// A sql step produces a `StreamList` anywhere inside the block.
    ///
    /// Rejected unconditionally (no consumer step needs to be present); serves
    /// the requirement "Load-time validation is fail-closed and typed".
    #[error(
        "sql step '{step}' uses outputType=StreamList inside a transacted block \
         on datasource '{datasource}'"
    )]
    StreamListInBlock {
        /// The sql step producing the stream.
        step: String,
        /// Datasource of the enclosing transacted block.
        datasource: String,
    },

    /// A sql step inside the block targets a different datasource.
    ///
    /// Serves the requirement "Load-time validation is fail-closed and typed"
    /// (no 2PC across datasources).
    #[error(
        "sql step '{step}' targets datasource '{step_datasource}' but the transacted block \
         is on datasource '{block_datasource}' (no two-phase commit)"
    )]
    ForeignDatasource {
        /// The sql step on the foreign datasource.
        step: String,
        /// Datasource of the enclosing transacted block.
        block_datasource: String,
        /// Datasource the offending sql step targets.
        step_datasource: String,
    },

    /// A sql step inside the block has no named datasource.
    ///
    /// Inline `db_url` cannot join a block-owned transaction; serves the
    /// requirement "Load-time validation is fail-closed and typed".
    #[error(
        "sql step '{step}' inside a transacted block has no named datasource \
         (inline db_url cannot join the block transaction)"
    )]
    UnnamedDatasource {
        /// The sql step missing the `datasource` parameter.
        step: String,
    },

    /// A `transacted` block is nested inside another one.
    ///
    /// Serves the requirement "Load-time validation is fail-closed and typed".
    #[error("transacted block on datasource '{inner}' is nested inside another transacted block")]
    NestedBlock {
        /// Datasource of the inner (nested) block.
        inner: String,
    },

    /// Joined SQL was refused because the session is aborted.
    ///
    /// Serves the requirement "Handled joined DB errors are rollback-only":
    /// after the database aborted the transaction, no further statement may run.
    #[error("transaction session on datasource '{datasource}' was aborted")]
    SessionAborted {
        /// Datasource of the aborted session.
        datasource: String,
    },

    /// The block body returned `Ok` after a joined DB error was handled.
    ///
    /// Serves the requirement "Handled joined DB errors are rollback-only":
    /// the server may have aborted the transaction, so a commit path must never
    /// report `Committed`.
    #[error(
        "transacted block on datasource '{datasource}' handled a joined database error; \
         the session is rollback-only"
    )]
    AbortedByHandledError {
        /// Datasource of the rollback-only session.
        datasource: String,
    },

    /// A `COMMIT` acknowledgment was lost or unreadable.
    ///
    /// Serves the requirement "Cancellation cutoff": an unacknowledged `COMMIT`
    /// is reported as uncertain and never as a rollback.
    #[error(
        "commit on datasource '{datasource}' is uncertain: the database did not acknowledge it"
    )]
    CommitUncertain {
        /// Datasource whose commit is uncertain.
        datasource: String,
    },

    /// The database rejected the `COMMIT`.
    ///
    /// Serves the requirement "Cancellation cutoff": a rejected `COMMIT` is a
    /// typed failure, never a reported rollback.
    #[error("commit on datasource '{datasource}' failed: {reason}")]
    CommitFailed {
        /// Datasource whose commit failed.
        datasource: String,
        /// Driver-reported failure detail.
        reason: String,
    },

    /// A `ROLLBACK` could not be acknowledged.
    ///
    /// Serves the requirement "Uncertain finalization detaches the connection":
    /// the connection is discarded and the outcome is `Detached`.
    #[error(
        "rollback on datasource '{datasource}' is uncertain: the database did not acknowledge it"
    )]
    RollbackUncertain {
        /// Datasource whose rollback is uncertain.
        datasource: String,
    },

    /// Connection acquisition exhausted the pool within the acquire deadline.
    ///
    /// Serves the requirement "Pool exhaustion is bounded and named".
    #[error(
        "no connection available for datasource '{datasource}' after waiting {waited_ms} ms \
         (max_connections={max_connections})"
    )]
    PoolExhausted {
        /// Datasource whose pool is exhausted.
        datasource: String,
        /// Configured pool size at the time of the failure.
        max_connections: u32,
        /// Time spent waiting for a connection.
        waited_ms: u64,
    },

    /// Block execution exceeded its deadline.
    ///
    /// Serves the requirement "Block timeout".
    #[error("transacted block on datasource '{datasource}' timed out after {elapsed_ms} ms")]
    Timeout {
        /// Datasource of the timed-out block.
        datasource: String,
        /// Time elapsed before the deadline fired.
        elapsed_ms: u64,
    },

    /// The session's single execution slot is already held.
    ///
    /// Serves the requirement "Busy block session" (at most one concurrent join
    /// per session).
    #[error("transaction session on datasource '{datasource}' is busy")]
    Busy {
        /// Datasource of the busy session.
        datasource: String,
    },

    /// A baked join has no transaction session to join at runtime.
    ///
    /// Serves the requirement "Join semantics for sql steps on the block
    /// datasource"; the step must fail rather than auto-commit.
    #[error("joined sql step on datasource '{datasource}' has no transaction session")]
    JoinSessionMissing {
        /// Datasource the step expected to join.
        datasource: String,
    },

    /// A baked join resolved to a different transaction scope at runtime.
    ///
    /// Serves the requirement "Join semantics for sql steps on the block
    /// datasource"; scope membership is lexical, so a mismatch is a hard error.
    #[error(
        "transaction join scope mismatch on datasource '{datasource}': \
         expected scope {expected_scope}, found scope {found_scope}"
    )]
    JoinScopeMismatch {
        /// Datasource the step expected to join.
        datasource: String,
        /// Scope id baked at load time.
        expected_scope: u64,
        /// Scope id of the session actually found at runtime.
        found_scope: u64,
    },

    /// A joined sql step's SQL comes from a runtime source (header/body/file).
    ///
    /// Serves the requirement "Join semantics for sql steps on the block
    /// datasource": a joined step's SQL is static, route-configured text.
    #[error("joined sql step '{step}' uses a runtime SQL source")]
    RuntimeSqlInJoinedStep {
        /// The step whose SQL is not statically known.
        step: String,
    },

    /// A joined sql step issues a transaction-control or implicit-commit statement.
    ///
    /// Serves the requirement "Join semantics for sql steps on the block
    /// datasource" (`BEGIN`/`COMMIT`/`ROLLBACK`/`SAVEPOINT` and equivalents
    /// inside a joined step are rejected at load).
    #[error("joined sql step '{step}' issues transaction-control statement '{statement}'")]
    TransactionControlSql {
        /// The offending sql step.
        step: String,
        /// The statement text that was rejected.
        statement: String,
    },

    /// The loader cannot prove the transaction semantics of a URL scheme.
    ///
    /// Serves the requirement "Join semantics for sql steps on the block
    /// datasource": dialect identity comes from the URL scheme, so an
    /// unrecognized scheme fails closed.
    #[error("cannot prove transaction semantics for URL scheme '{scheme}'")]
    DialectUnprovable {
        /// URL scheme whose dialect could not be resolved.
        scheme: String,
    },

    /// The resolved dialect does not support joined transactions in v1.
    ///
    /// Serves the requirement "Join semantics for sql steps on the block
    /// datasource": joined steps are PostgreSQL-only in v1.
    #[error(
        "dialect '{dialect}' on datasource '{datasource}' does not support joined transactions"
    )]
    UnsupportedJoinDialect {
        /// Resolved dialect name (e.g. `mysql`, `sqlite`).
        dialect: String,
        /// Datasource of the offending block.
        datasource: String,
    },

    /// An in-block dynamic routing target was rejected at load.
    ///
    /// Serves the requirement "Join semantics for sql steps on the block
    /// datasource": a step whose destination is computed from the exchange
    /// (recipient list, routing slip, dynamic router) cannot prove block
    /// membership statically, so a transacted block containing one is
    /// rejected at load.
    #[error(
        "dynamic routing target at step '{step}' inside a transacted block \
         cannot prove transaction membership"
    )]
    DynamicTargetUnprovable {
        /// The dynamic-routing step inside the block.
        step: String,
    },
}

/// Boxed future resolving to a new [`TransactionSession`].
///
/// Hand-typed like [`crate::datasource::GetPoolFuture`] so the trait stays
/// object safe without an `async_trait` dependency.
pub type OpenSessionFuture<'a> = Pin<
    Box<dyn Future<Output = Result<Arc<dyn TransactionSession>, TransactionError>> + Send + 'a>,
>;

/// Boxed future for the lazy `BEGIN` of a [`TransactionSession`].
pub type BeginFuture<'a> = Pin<Box<dyn Future<Output = Result<(), TransactionError>> + Send + 'a>>;

/// Boxed future for finalizing a [`TransactionSession`].
///
/// Resolves to the observed [`TransactionOutcome`] so the caller can
/// distinguish an acknowledged commit/rollback from a detached connection.
pub type FinalizeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<TransactionOutcome, TransactionError>> + Send + 'a>>;

/// Provider-neutral factory for transaction sessions on one datasource.
///
/// Implemented by the sql component (slice 2) and resolved by datasource
/// name from the `camel-core` registry. `camel-api` never sees a driver or a
/// SQL value model: slice 2 keeps statement execution in its own extension
/// trait over the concrete session type.
///
/// `Send + Sync + 'static` lets the registry hold the manager in an `Arc` and
/// share it across workers. [`TransactionManager::open_session`] returns a
/// boxed future so the trait object is object safe. The [`Any`] supertrait
/// lets boot-time registration tests recover the concrete manager from the
/// registry by trait-object upcast, exactly like [`TransactionSession`]; it is
/// auto-implemented, so implementors carry no extra burden.
pub trait TransactionManager: Any + Send + Sync + 'static {
    /// Datasource name this manager serves.
    ///
    /// The registry keys managers by this name, and load validation reports it
    /// in every typed error.
    fn datasource(&self) -> &str;

    /// Open a session on this manager's datasource.
    ///
    /// Opening borrows a pooled connection but SHALL NOT send `BEGIN`: begin is
    /// lazy and driven by [`TransactionSession::begin`] at the first joined sql
    /// step. Registry resolution failures surface as
    /// [`TransactionError::ManagerMissing`] before this method is reached.
    fn open_session(&self) -> OpenSessionFuture<'_>;

    /// Bound for connection acquisition, normally the datasource
    /// `acquire_timeout`.
    ///
    /// Slice 2 compares this deadline with the block timeout and reports the
    /// deadline that elapses first (requirement "Pool exhaustion is bounded and
    /// named", "Block timeout").
    fn acquire_deadline(&self) -> Duration;
}

/// One block-owned transaction on one datasource.
///
/// A session owns (or guards) its pooled connection for the whole block, so
/// every joined sql step on the block datasource shares one transaction
/// (requirement "Join semantics for sql steps on the block datasource"). The
/// session is `'static` so the implementing crate can downcast the trait object
/// to its concrete type for statement execution without `camel-api` gaining a
/// SQL value model.
///
/// The [`Any`] supertrait makes that downcast a trait-object upcast: an
/// `Arc<dyn TransactionSession>` coerces to `Arc<dyn Any + Send + Sync>` and
/// [`Arc::downcast`](std::sync::Arc::downcast) recovers the concrete session
/// type. `Any` is auto-implemented, so implementors carry no extra burden.
pub trait TransactionSession: Any + Send + Sync + 'static {
    /// Datasource this session runs on.
    fn datasource(&self) -> &str;

    /// Current lifecycle state.
    ///
    /// Slice 2 relies on the explicit machine (`Fresh → Active → Finalizing →
    /// Done | Detached`) rather than inferring state from a borrow.
    fn state(&self) -> TransactionState;

    /// Send the lazy `BEGIN`.
    ///
    /// Called at the first joined sql step, not when the session opens.
    /// Precondition `Clean`; `Ok(())` if already `Open` (idempotent);
    /// `Err(SessionAborted)` otherwise. A begin that arrives while the
    /// session's execution slot is held is rejected with the typed `Busy`
    /// error. The owner SHALL call `begin` only
    /// from the `Clean` state or to observe an already-open session, and
    /// admission hygiene must have been acknowledged before the first
    /// `BEGIN` (requirement "Lazy begin and block-owned commit/rollback").
    fn begin(&self) -> BeginFuture<'_>;

    /// Mark the session rollback-only.
    ///
    /// Slice 2 calls this before returning a joined database error to the
    /// pipeline (requirement "Handled joined DB errors are rollback-only"), so a
    /// handled error can never commit a server-aborted transaction.
    fn set_rollback_only(&self);

    /// Whether the session has been marked rollback-only.
    fn is_rollback_only(&self) -> bool;

    /// Commit the block's transaction.
    ///
    /// Returns the observed [`TransactionOutcome`]; slice 2 relies on the
    /// outcome (not just `Ok`) to distinguish an acknowledged commit from a
    /// detached connection (requirement "Cancellation cutoff", "Uncertain
    /// finalization detaches the connection").
    ///
    /// Precondition `Active`. Otherwise no SQL is sent and the call returns
    /// `Err(SessionAborted)`. The owner SHALL call `commit` only when
    /// `state() == Active`.
    fn commit(&self) -> FinalizeFuture<'_>;

    /// End the block without committing. Total over every lifecycle state; the
    /// implementation selects the action from its private phase:
    /// - nothing begun (`Fresh`, BEGIN never sent): no SQL, connection returned
    ///   clean, `Ok(RolledBack)` (nothing was begun, so nothing is durable);
    /// - BEGIN sent but unacknowledged (`Fresh`): no SQL, detach,
    ///   `Err(RollbackUncertain)`;
    /// - `Active`: ROLLBACK within the finalize grace; acknowledged →
    ///   `Ok(RolledBack)`, else detach + `Err(RollbackUncertain)`;
    /// - `Detached`, or a defensive `Finalizing`/`Done` entry: no SQL,
    ///   `Err(RollbackUncertain)` (`error!` log for the defensive entry).
    fn rollback(&self) -> FinalizeFuture<'_>;
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    /// Minimal concrete port implementation used by the port tests.
    ///
    /// Every method resolves without I/O: the tests exercise the trait-object
    /// surface (upcast/downcast) and the doc contract, not execution.
    pub(super) struct StubSession {
        datasource: String,
        state: TransactionState,
        rollback_only: AtomicBool,
    }

    impl StubSession {
        pub(super) fn new(datasource: &str) -> Self {
            Self {
                datasource: datasource.to_owned(),
                state: TransactionState::Active,
                rollback_only: AtomicBool::new(false),
            }
        }
    }

    impl TransactionSession for StubSession {
        fn datasource(&self) -> &str {
            &self.datasource
        }

        fn state(&self) -> TransactionState {
            self.state
        }

        fn begin(&self) -> BeginFuture<'_> {
            Box::pin(std::future::ready(Ok(())))
        }

        fn set_rollback_only(&self) {
            self.rollback_only.store(true, Ordering::Relaxed);
        }

        fn is_rollback_only(&self) -> bool {
            self.rollback_only.load(Ordering::Relaxed)
        }

        fn commit(&self) -> FinalizeFuture<'_> {
            Box::pin(std::future::ready(Ok(TransactionOutcome::Committed)))
        }

        fn rollback(&self) -> FinalizeFuture<'_> {
            Box::pin(std::future::ready(Ok(TransactionOutcome::RolledBack)))
        }
    }

    // Compile-time proof that both port traits are usable as trait objects.
    #[test]
    fn port_traits_are_object_safe() {
        fn _assert(_m: &dyn TransactionManager, _s: &dyn TransactionSession) {}
    }

    #[test]
    fn session_port_upcasts_to_any_and_downcasts() {
        let concrete = Arc::new(StubSession::new("pg"));
        let port: Arc<dyn TransactionSession> = concrete.clone();

        // The `Any` supertrait makes the erase explicit: a trait-object upcast
        // to `dyn Any`, no wrapper and no per-implementor method.
        let erased: Arc<dyn Any + Send + Sync> = port;

        let restored = Arc::downcast::<StubSession>(erased)
            .expect("the session trait object must downcast to its concrete type");
        assert_eq!(restored.datasource(), "pg");
        assert!(
            Arc::ptr_eq(&concrete, &restored),
            "downcast must recover the same allocation, not a copy"
        );
    }

    #[test]
    fn rollback_doc_carries_total_contract() {
        // Source-text assertion: the 604d contract is docs-only, so the review
        // gate is the doc text itself. Only the production half of the file is
        // searched: the phrases below also appear as literals in this test's
        // own arrays, so a whole-file read would pass with every `///` doc
        // stripped. `///` / `//!` markers are removed line-wise and whitespace
        // is normalised so the test tracks prose, not the exact doc wrapping.
        let source =
            std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/transaction.rs"))
                .expect("camel-api source tree is present at test time");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .unwrap_or("")
            .lines()
            .map(|line| {
                let line = line.trim();
                line.strip_prefix("///")
                    .or_else(|| line.strip_prefix("//!"))
                    .unwrap_or(line)
            })
            .collect::<Vec<_>>()
            .join(" ");
        let production = production.split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            !production.is_empty(),
            "the production half of transaction.rs must be non-empty"
        );

        for phrase in [
            "Total over every lifecycle state",
            "nothing begun (`Fresh`, BEGIN never sent)",
            "BEGIN sent but unacknowledged",
            "`Active`: ROLLBACK within the finalize grace",
            "`Detached`, or a defensive `Finalizing`/`Done` entry",
            "`error!` log for the defensive entry",
        ] {
            assert!(
                production.contains(phrase),
                "rollback doc must carry the total-rollback contract phrase: {phrase}"
            );
        }

        for phrase in [
            "Precondition `Active`. Otherwise no SQL is sent and the call returns `Err(SessionAborted)`.",
            "The owner SHALL call `commit` only when `state() == Active`.",
        ] {
            assert!(
                production.contains(phrase),
                "commit doc must carry the precondition phrase: {phrase}"
            );
        }

        // The 604d contract replaces the pre-604d prose; a retained copy would
        // mean the replacement was an append and the port would carry two
        // contradictory rollback contracts.
        assert!(
            !production.contains("Roll back the block's transaction."),
            "the pre-604d rollback doc must be replaced, not kept alongside the new contract"
        );
    }

    #[test]
    fn transaction_error_display_names_offender_and_datasource() {
        let manager_missing = TransactionError::ManagerMissing {
            datasource: "pg".into(),
        };
        let rendered = manager_missing.to_string();
        assert!(
            rendered.contains("pg") && rendered.contains("TransactionManager"),
            "manager-missing display must name the datasource and the missing port: {rendered}"
        );

        let foreign = TransactionError::ForeignDatasource {
            step: "s1".into(),
            block_datasource: "pg".into(),
            step_datasource: "audit".into(),
        };
        let rendered = foreign.to_string();
        assert!(
            rendered.contains("s1") && rendered.contains("pg") && rendered.contains("audit"),
            "foreign-datasource display must name the step and both datasources: {rendered}"
        );

        let stream_list = TransactionError::StreamListInBlock {
            step: "s2".into(),
            datasource: "pg".into(),
        };
        let rendered = stream_list.to_string();
        assert!(
            rendered.contains("s2") && rendered.contains("pg"),
            "stream-list display must name the step and the datasource: {rendered}"
        );
    }

    #[test]
    #[allow(clippy::match_single_binding)]
    fn non_exhaustive_enums_accept_catch_all() {
        // `#[non_exhaustive]` keeps downstream crates from writing an exhaustive
        // match, so a catch-all arm alone must compile for both enums.
        let state = TransactionState::Fresh;
        let outcome = TransactionOutcome::Committed;

        let state_label = match state {
            _ => "state",
        };
        let outcome_label = match outcome {
            _ => "outcome",
        };

        assert_eq!(state_label, "state");
        assert_eq!(outcome_label, "outcome");
    }
}
