//! Runtime join channels that bind a joined sql step to its block session.
//!
//! A `transacted { datasource }` block owns exactly one
//! [`TransactionSession`]; every joined sql step inside the block must run on
//! that session rather than auto-commit on a fresh connection. The binding is
//! carried in two task-local cells because the pipeline runs a step on the
//! block's own task, while the load-compiled step cannot hold a runtime
//! `Arc`:
//!
//! - [`BLOCK_SESSION`] holds the [`BlockSession`] a segment installs for its
//!   body task: the datasource, the scope id of the enclosing block, and the
//!   session trait object;
//! - [`JOIN_SCOPE`] holds the scope id baked into a joined sql step at load
//!   time.
//!
//! A joined sql step reads both cells, compares its baked [`JoinScope`] with
//! the block's scope id, and only then joins. Scope membership is lexical, so
//! a mismatch is a hard error rather than a silent join into another block's
//! transaction.
//!
//! The cells are task-local on purpose: sibling branches of a split would
//! otherwise observe the same session, which the port forbids (at most one
//! concurrent join per session). Nothing here knows about `sqlx` or the SQL
//! value model; the session stays a [`TransactionSession`] trait object that
//! the sql component downcasts to its concrete type.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use super::TransactionSession;

tokio::task_local! {
    /// Block session installed by the enclosing transacted segment.
    ///
    /// Present only for the body task of a transacted block; absent means the
    /// step runs outside any block and must not join. Read it through
    /// [`current_block_session`] or install it with [`BlockSession::enter`].
    pub static BLOCK_SESSION: BlockSession;

    /// Join scope id baked into the current sql step at load time.
    ///
    /// Baked per joined step, not per block: the step compares this id with
    /// [`BlockSession::scope`] to prove it is joining its own block. Read it
    /// through [`JoinScope::current`].
    pub static JOIN_SCOPE: u64;
}

/// The datasource, scope id, and session of one running transacted block.
///
/// A segment builds one per block entry and installs it for the body task
/// through [`BlockSession::enter`]; joined steps then read it with
/// [`current_block_session`]. Cloning is cheap: the datasource is shared and
/// the session is an `Arc` (the block keeps its own handle for commit or
/// rollback).
#[derive(Clone)]
pub struct BlockSession {
    datasource: Arc<str>,
    scope: JoinScope,
    session: Arc<dyn TransactionSession>,
}

impl BlockSession {
    /// Build the binding for a block on `datasource` in scope `scope`.
    ///
    /// `session` is the block's owned session handle, the same allocation the
    /// block later commits or rolls back.
    #[must_use]
    pub fn new(
        datasource: impl Into<Arc<str>>,
        scope: JoinScope,
        session: Arc<dyn TransactionSession>,
    ) -> Self {
        Self {
            datasource: datasource.into(),
            scope,
            session,
        }
    }

    /// Datasource the block is transacted on.
    #[must_use]
    pub fn datasource(&self) -> &str {
        &self.datasource
    }

    /// Scope id of the enclosing block.
    ///
    /// A joined step compares this with its own baked [`JoinScope`]; equality
    /// proves the step belongs to this block.
    #[must_use]
    pub fn scope(&self) -> JoinScope {
        self.scope
    }

    /// The block's transaction session.
    ///
    /// Returned as a clone of the shared handle; the caller must still go
    /// through the port (lazy `begin`, rollback-only marking) and must not
    /// finalize it.
    #[must_use]
    pub fn session(&self) -> Arc<dyn TransactionSession> {
        Arc::clone(&self.session)
    }

    /// Install `self` as [`BLOCK_SESSION`] for the task that polls `fut`.
    ///
    /// The binding ends when `fut` completes or is dropped, so nested blocks
    /// shadow and unwrap without touching the outer binding.
    pub async fn enter<F: Future>(self, fut: F) -> F::Output {
        BLOCK_SESSION.scope(self, fut).await
    }
}

impl fmt::Debug for BlockSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlockSession")
            .field("datasource", &self.datasource)
            .field("scope", &self.scope)
            .field("state", &self.session.state())
            .finish_non_exhaustive()
    }
}

/// Block scope id, baked per joined sql step and compared at join time.
///
/// The id is opaque: the loader assigns one per transacted block, bakes it
/// into every joined step of that block, and the segment uses the same id when
/// it installs the [`BlockSession`]. A step whose baked id differs from the
/// running block's id is a [`super::TransactionError::JoinScopeMismatch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JoinScope(u64);

impl JoinScope {
    /// Wrap a loader-assigned scope id.
    #[must_use]
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    /// The raw scope id (for the typed error fields, which carry `u64`).
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Install `self` as [`JOIN_SCOPE`] for the task that polls `fut`.
    ///
    /// Used by a joined sql step to bake its load-time scope for work it
    /// spawns, and by tests; the runtime segment installs [`BLOCK_SESSION`],
    /// not this cell.
    pub async fn set<F: Future>(self, fut: F) -> F::Output {
        JOIN_SCOPE.scope(self.0, fut).await
    }

    /// The scope id baked into the current task, if any.
    ///
    /// `None` means the current task carries no baked scope: the step is not a
    /// joined step, so it must not join a transaction.
    #[must_use]
    pub fn current() -> Option<Self> {
        JOIN_SCOPE.try_with(|scope| Self(*scope)).ok()
    }

    /// Whether the current task's baked scope equals `self`.
    ///
    /// Shorthand for the join-time comparison: an absent scope is never a
    /// match, so a non-joined step fails closed.
    #[must_use]
    pub fn scope_is(self) -> bool {
        Self::current() == Some(self)
    }
}

/// The [`BlockSession`] installed for the current task, if any.
///
/// `None` means the current task runs outside a transacted block body; a
/// joined step must then fail with
/// [`super::TransactionError::JoinSessionMissing`] instead of auto-committing.
#[must_use]
pub fn current_block_session() -> Option<BlockSession> {
    BLOCK_SESSION.try_with(BlockSession::clone).ok()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::transaction::tests::StubSession;

    fn session() -> Arc<dyn TransactionSession> {
        Arc::new(StubSession::new("pg"))
    }

    #[tokio::test]
    async fn join_task_local_scopes_isolate() {
        let outer = BlockSession::new("pg", JoinScope::new(1), session());
        let inner = BlockSession::new("pg", JoinScope::new(2), session());

        assert!(
            current_block_session().is_none(),
            "no block session is installed outside a block body"
        );

        outer
            .clone()
            .enter(async {
                let running = current_block_session().expect("outer scope installs the session");
                assert_eq!(running.scope(), JoinScope::new(1));
                assert_eq!(running.datasource(), "pg");

                inner
                    .clone()
                    .enter(async {
                        let running =
                            current_block_session().expect("inner scope installs the session");
                        assert_eq!(
                            running.scope(),
                            JoinScope::new(2),
                            "inner scope shadows the outer one inside its own task"
                        );
                    })
                    .await;

                let running =
                    current_block_session().expect("the outer binding survives the inner scope");
                assert_eq!(
                    running.scope(),
                    JoinScope::new(1),
                    "the inner scope must not leak past its future"
                );
            })
            .await;

        assert!(
            current_block_session().is_none(),
            "the binding ends with the block body"
        );

        // A spawned task starts with no inherited cell: joining is lexical,
        // never ambient across a producer handoff.
        outer
            .enter(async {
                tokio::spawn(async {
                    assert!(
                        current_block_session().is_none(),
                        "a spawned task must not inherit a block session"
                    );
                })
                .await // allow-test-wait: join of the test-spawned finite task; purely in-memory
                .expect("spawned task completes");
            })
            .await;
    }

    #[tokio::test]
    async fn join_scope_set_and_compare() {
        assert_eq!(
            JoinScope::current(),
            None,
            "no scope is baked outside a joined step"
        );
        assert!(!JoinScope::new(3).scope_is(), "absent scope never matches");

        JoinScope::new(3)
            .set(async {
                assert_eq!(JoinScope::current(), Some(JoinScope::new(3)));
                assert!(JoinScope::new(3).scope_is());
                assert!(!JoinScope::new(4).scope_is());
            })
            .await;

        assert_eq!(
            JoinScope::current(),
            None,
            "the baked scope ends with its future"
        );
        assert_eq!(JoinScope::new(9).get(), 9);
    }
}
