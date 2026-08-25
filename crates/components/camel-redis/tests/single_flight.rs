//! Single-flight (connect-storm collapse) tests for the
//! `MultiplexedExecutor` connect gate (tasks 2.1 and 2.2,
//! redis-single-flight).
//!
//! Exercises the public surface only: `MultiplexedExecutor::new`,
//! `get_conn`, `refresh`, `RedisEndpointConfig::from_uri`,
//! `topology_from_config`, and the `RedisTopology` trait (via a
//! counting decorator).
//!
//! Determinism argument (why `count == 1` is exact, not probabilistic):
//!
//! - Every test runs on a `current_thread` runtime, so a spawned task
//!   runs from its first statement to its first pending await without
//!   preemption. `yield_now()` in the main task's poll loop lets the
//!   whole herd run to its park point (gate lock or held handshake)
//!   before `release()` is ever called.
//! - The herd's started-counter increments as each member's FIRST
//!   statement, and between that increment and the park point there is
//!   no pending await on an empty cache (the `conn` mutex is
//!   uncontended, the gate lock is immediate for the first member, and
//!   `resolve` is synchronous underneath its async signature). The main
//!   task therefore observes `started == n` only when every member is
//!   already parked where the scenario needs it.
//! - The hold gate (see `common/mod.rs`) parks the leader's connect
//!   mid-handshake, freezing the single-flight gate's leadership until
//!   the test releases it.
//!
//! Liveness: every herd and the leader/waiter choreography are wrapped
//! in a 30s `tokio::time::timeout` that panics on elapse — a regression
//! that deadlocks members on the gate fails loudly instead of hanging
//! CI. No wall-clock sleeps; the polls are `yield_now`-based.
//!
//! The returned connections are NEVER pinged: the silent stub consumes
//! application commands without a reply, so a PING await would hang
//! until its response deadline.

mod common;

use async_trait::async_trait;
use camel_api::CamelError;
use camel_component_redis::{
    MultiplexedExecutor, RedisEndpointConfig, RedisTopology, ServerKind, topology_from_config,
};
use common::{HandshakeStub, HandshakeStubHandle};
use futures_util::future::join_all;
use redis::aio::MultiplexedConnection;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

/// Liveness bound for herds and choreographies. Everything in these
/// tests completes in microseconds once the hold gate releases; this
/// bound only converts a gate deadlock into a loud panic.
const LIVENESS_BOUND: Duration = Duration::from_secs(30);

// ------------------------------------------------------------------
// Counting topology: every resolve through the executor is counted,
// including the pre-build connect (the topology is installed at
// executor construction, so no resolve can escape the counter).
// ------------------------------------------------------------------

/// `RedisTopology` decorator that counts resolve calls, then delegates
/// every trait method to the wrapped topology.
struct CountingTopology {
    inner: Arc<dyn RedisTopology>,
    count: Arc<AtomicUsize>,
}

#[async_trait]
impl RedisTopology for CountingTopology {
    async fn resolve(&self, kind: ServerKind) -> Result<redis::Client, CamelError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        self.inner.resolve(kind).await
    }
}

/// Standalone executor for `addr` whose topology resolve calls land in
/// `count`.
fn counting_executor(addr: SocketAddr, count: Arc<AtomicUsize>) -> MultiplexedExecutor {
    let config = RedisEndpointConfig::from_uri(&format!("redis://{addr}"))
        .expect("valid standalone URI for the stub");
    let topology = Arc::new(CountingTopology {
        inner: topology_from_config(&config).expect("standalone topology"),
        count,
    });
    MultiplexedExecutor::new(config, topology)
}

// ------------------------------------------------------------------
// Herd machinery
// ------------------------------------------------------------------

/// Which `MultiplexedExecutor` call a herd member performs. An enum
/// (rather than a generic closure) keeps the herd helper concrete: the
/// calls return futures borrowing the executor, so they cannot satisfy
/// a `Fn() -> Fut` bound without boxing.
#[derive(Clone, Copy)]
enum HerdCall {
    /// `get_conn` — the cold-start path.
    GetConn,
    /// `refresh` — cache clear + `get_conn`.
    Refresh,
}

impl HerdCall {
    async fn run(
        self,
        executor: &MultiplexedExecutor,
    ) -> Result<MultiplexedConnection, CamelError> {
        match self {
            HerdCall::GetConn => executor.get_conn().await,
            HerdCall::Refresh => executor.refresh().await,
        }
    }
}

/// Poll (yield-based, no wall-clock sleeps) until `started` reaches
/// `target`. On a `current_thread` runtime each yield lets a spawned
/// task advance to its next park point, so the loop terminates once the
/// observed tasks have run their first statement.
async fn poll_started(started: &AtomicUsize, target: usize) {
    loop {
        if started.load(Ordering::SeqCst) == target {
            return;
        }
        tokio::task::yield_now().await;
    }
}

/// Spawn `herd_size` members that all run `call` on clones of one
/// shared executor, wait until every member has ENTERED its executor
/// call, release the stub's hold gate, then join all members and return
/// their results in spawn order.
///
/// Shape: per-call enum dispatch (`HerdCall`) instead of a generic
/// closure — see `HerdCall` docs. The whole herd (spawn, readiness
/// poll, release, join) is bounded by [`LIVENESS_BOUND`]; a timeout
/// panics.
async fn run_herd(
    call: HerdCall,
    herd_size: usize,
    stub: &HandshakeStubHandle,
    executor: MultiplexedExecutor,
) -> Vec<Result<MultiplexedConnection, CamelError>> {
    let herd = async {
        let started = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::with_capacity(herd_size);
        for _ in 0..herd_size {
            let started = Arc::clone(&started);
            let executor = executor.clone();
            handles.push(tokio::spawn(async move {
                started.fetch_add(1, Ordering::SeqCst);
                call.run(&executor).await
            }));
        }

        poll_started(&started, herd_size).await;
        stub.release();

        join_all(handles)
            .await
            .into_iter()
            .map(|joined| joined.expect("herd member must not panic"))
            .collect()
    };

    tokio::time::timeout(LIVENESS_BOUND, herd)
        .await
        .expect("herd must complete within the 30s liveness bound (a member deadlocked?)")
}

// ------------------------------------------------------------------
// Tests
// ------------------------------------------------------------------

#[tokio::test(flavor = "current_thread")]
async fn concurrent_refresh_collapses_to_one_resolve() {
    let (addr, stub) = HandshakeStub::start_silent().expect("bind silent stub");
    let count = Arc::new(AtomicUsize::new(0));
    let executor = counting_executor(addr, Arc::clone(&count));

    // Pre-build in real time: the unheld stub answers the handshake, so
    // the executor caches a live connection.
    executor
        .get_conn()
        .await
        .expect("pre-build connect must succeed against the unheld stub");

    // Isolate the herd phase: everything below builds fresh.
    count.store(0, Ordering::SeqCst);
    stub.hold();

    let results = run_herd(HerdCall::Refresh, 5, &stub, executor).await;

    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "5 concurrent refresh calls must collapse to exactly one topology resolve \
         (pre-change this would be 5)"
    );
    assert!(
        results.iter().all(|r| r.is_ok()),
        "every herd member must receive a connection"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cold_start_get_conn_collapses_to_one() {
    let (addr, stub) = HandshakeStub::start_silent_held().expect("bind held silent stub");
    let count = Arc::new(AtomicUsize::new(0));
    let executor = counting_executor(addr, Arc::clone(&count));

    // Cold start: no pre-build, no counter reset — the first resolve
    // this executor ever performs is the herd's.
    let results = run_herd(HerdCall::GetConn, 5, &stub, executor).await;

    assert_eq!(
        count.load(Ordering::SeqCst),
        1,
        "5 concurrent cold-start get_conn calls must collapse to exactly one \
         topology resolve"
    );
    assert!(
        results.iter().all(|r| r.is_ok()),
        "every herd member must receive a connection"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dropped_leader_releases_gate_and_waiter_proceeds() {
    let (addr, stub) = HandshakeStub::start_silent_held().expect("bind held silent stub");
    let count = Arc::new(AtomicUsize::new(0));
    let executor = counting_executor(addr, Arc::clone(&count));

    let (waiter_result, resolves) = tokio::time::timeout(LIVENESS_BOUND, async {
        // Leader: enters get_conn, resolves (counted — resolve precedes
        // connect), then parks in the held handshake while holding the
        // connect gate.
        let leader_started = Arc::new(AtomicUsize::new(0));
        let leader = {
            let started = Arc::clone(&leader_started);
            let executor = executor.clone();
            tokio::spawn(async move {
                started.fetch_add(1, Ordering::SeqCst);
                executor.get_conn().await
            })
        };
        poll_started(&leader_started, 1).await;

        // Waiter: enters get_conn, misses the (empty) cache, and parks
        // on the connect gate the leader holds.
        let waiter_started = Arc::new(AtomicUsize::new(0));
        let waiter = {
            let started = Arc::clone(&waiter_started);
            let executor = executor.clone();
            tokio::spawn(async move {
                started.fetch_add(1, Ordering::SeqCst);
                executor.get_conn().await
            })
        };
        poll_started(&waiter_started, 1).await;

        // Drop the leader mid-connect: its gate guard must be released
        // with it, letting the waiter take over as the new leader once
        // the stub answers handshakes again.
        leader.abort();
        stub.release();

        let waiter_result = waiter.await.expect("waiter task must not panic");
        (waiter_result, count.load(Ordering::SeqCst))
    })
    .await
    .expect("choreography must complete within the 30s liveness bound (gate never released?)");

    assert!(
        waiter_result.is_ok(),
        "waiter must build a connection after the aborted leader releases the gate"
    );
    assert_eq!(
        resolves, 2,
        "exactly two resolves: the aborted leader's (resolve precedes connect, so it \
         counted before the abort) plus the waiter's own leader build"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn straggler_invalidation_forces_one_sequential_rebuild() {
    let (addr, stub) = HandshakeStub::start_silent().expect("bind silent stub");
    let count = Arc::new(AtomicUsize::new(0));
    let executor = counting_executor(addr, Arc::clone(&count));

    // Pre-build in real time: the unheld stub answers the handshake, so
    // the executor caches a live connection.
    executor
        .get_conn()
        .await
        .expect("pre-build connect must succeed against the unheld stub");

    // Isolate the rebuild phase: everything below builds fresh.
    count.store(0, Ordering::SeqCst);
    stub.hold();

    let (leader_result, straggler_result, resolves) = tokio::time::timeout(LIVENESS_BOUND, async {
        // Leader: refresh clears the cache, then its get_conn
        // resolves (counted) and parks in the held handshake while
        // holding the connect gate.
        let leader_started = Arc::new(AtomicUsize::new(0));
        let leader = {
            let started = Arc::clone(&leader_started);
            let executor = executor.clone();
            tokio::spawn(async move {
                started.fetch_add(1, Ordering::SeqCst);
                executor.refresh().await
            })
        };
        poll_started(&leader_started, 1).await;

        // Release answers the held handshake, so the leader runs
        // resolve+connect+store to completion.
        stub.release();

        // JOIN the leader before the straggler exists: the
        // straggler's cache invalidation therefore lands strictly
        // AFTER the leader's store, by construction.
        let leader_result = leader.await.expect("leader task must not panic");

        // Straggler: a late refresher that invalidates the cache the
        // leader just stored. `release()` set the stub back to Free,
        // so its handshake is answered immediately; it must run its
        // own resolve+connect — one bounded, sequential rebuild.
        let straggler_started = Arc::new(AtomicUsize::new(0));
        let straggler = {
            let started = Arc::clone(&straggler_started);
            let executor = executor.clone();
            tokio::spawn(async move {
                started.fetch_add(1, Ordering::SeqCst);
                executor.refresh().await
            })
        };
        poll_started(&straggler_started, 1).await;

        let straggler_result = straggler.await.expect("straggler task must not panic");
        (
            leader_result,
            straggler_result,
            count.load(Ordering::SeqCst),
        )
    })
    .await
    .expect(
        "choreography must complete within the 30s liveness bound (leader or straggler stuck?)",
    );

    assert!(
        leader_result.is_ok(),
        "leader rebuild must succeed once the hold gate releases"
    );
    assert!(
        straggler_result.is_ok(),
        "straggler rebuild must succeed against the released (Free) stub"
    );
    assert_eq!(
        resolves, 2,
        "leader + straggler: a cache invalidation landing after the leader's store \
         forces exactly one extra sequential rebuild"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn waiter_does_not_inherit_leader_failure() {
    let (addr, stub) = HandshakeStub::start_rejecting_after_hold().expect("bind rejecting stub");
    let count = Arc::new(AtomicUsize::new(0));
    // Fresh executor, no pre-build: the first resolve this executor
    // ever performs is the herd's.
    let executor = counting_executor(addr, Arc::clone(&count));

    // The rejecting stub holds handshakes until release, so all three
    // members park (the leader mid-handshake holding the gate, two
    // waiters on the gate). release() closes the leader's connection
    // unreplied — the leader fails and releases the gate — and every
    // future connect is refused instantly, so each waiter's OWN attempt
    // fails fast instead of inheriting the leader's failure future.
    // The herd is therefore bounded by the leader's held-then-closed
    // connect, well inside run_herd's liveness bound.
    let results = run_herd(HerdCall::Refresh, 3, &stub, executor).await;

    assert!(
        results.iter().all(|r| r.is_err()),
        "leader and both waiters must observe their own connect failure (waiters \
         must not inherit the leader's error)"
    );
    assert_eq!(
        count.load(Ordering::SeqCst),
        3,
        "each caller ran its own resolve after the leader failed (a shared-future \
         design would show exactly 1)"
    );
}
