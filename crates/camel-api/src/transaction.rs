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
/// boxed future so the trait object is object safe.
pub trait TransactionManager: Send + Sync + 'static {
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
pub trait TransactionSession: Send + Sync + 'static {
    /// Datasource this session runs on.
    fn datasource(&self) -> &str;

    /// Current lifecycle state.
    ///
    /// Slice 2 relies on the explicit machine (`Fresh → Active → Finalizing →
    /// Done | Detached`) rather than inferring state from a borrow.
    fn state(&self) -> TransactionState;

    /// Send the lazy `BEGIN`.
    ///
    /// Called at the first joined sql step, not when the session opens. A second
    /// `begin` on the same session is an error, and admission hygiene must have
    /// been acknowledged before the first `BEGIN` (requirement "Lazy begin and
    /// block-owned commit/rollback").
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
    fn commit(&self) -> FinalizeFuture<'_>;

    /// Roll back the block's transaction.
    ///
    /// Returns the observed [`TransactionOutcome`]; a rollback that cannot be
    /// acknowledged is reported as [`TransactionError::RollbackUncertain`] and
    /// the connection is detached instead of returned to the pool.
    fn rollback(&self) -> FinalizeFuture<'_>;
}

#[cfg(test)]
mod tests {
    use super::*;

    // Compile-time proof that both port traits are usable as trait objects.
    #[test]
    fn port_traits_are_object_safe() {
        fn _assert(_m: &dyn TransactionManager, _s: &dyn TransactionSession) {}
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
