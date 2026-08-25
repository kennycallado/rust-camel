//! Driver-level response-deadline tests for `MultiplexedExecutor`'s
//! `with_response_timeout` builder (task 1.2, redis-response-timeout).
//!
//! Exercises the public surface only: `MultiplexedExecutor::new`,
//! `with_response_timeout`, `get_conn`, `refresh`,
//! `RedisEndpointConfig::from_uri`, and `topology_from_config`.
//!
//! The silent stub (see `common/mod.rs`) answers the driver handshake
//! (`CLIENT`/`SELECT`/`AUTH` → `+OK`) and then consumes every
//! application command without a reply, so a connected client's command
//! await pends until its configured response deadline fires. A
//! zero-byte silent peer is NOT enough: the driver completes
//! `setup_connection` before returning the connection, so the connect
//! would fail instead of the command deadline firing.
//!
//! Clock discipline: tests 1-3 connect in real time and call
//! `tokio::time::pause()` only for the measured query — `start_paused`
//! with pending real-socket handshakes lets virtual time auto-advance
//! spuriously past live timers. Test 4 has no real I/O that completes,
//! so `start_paused` is deterministic there.

mod common;

use camel_api::CamelError;
use camel_component_redis::{MultiplexedExecutor, RedisEndpointConfig, topology_from_config};
use common::{HandshakeStub, never_handshake_peer};
use std::net::SocketAddr;
use std::time::Duration;

// ------------------------------------------------------------------
// Executor under test, built through the public surface.
// ------------------------------------------------------------------

/// Standalone executor pointing at `addr` with the configured driver
/// response deadline. `connection_timeout_secs` stays at its default.
fn executor_for(addr: SocketAddr, response_timeout: Duration) -> MultiplexedExecutor {
    let config = RedisEndpointConfig::from_uri(&format!("redis://{addr}"))
        .expect("valid standalone URI for the stub");
    let topology = topology_from_config(&config).expect("standalone topology");
    MultiplexedExecutor::new(config, topology).with_response_timeout(response_timeout)
}

#[tokio::test]
async fn configured_large_timeout_outlives_driver_default() {
    let (addr, _stub) = HandshakeStub::start_silent().expect("bind silent stub");
    let executor = executor_for(addr, Duration::from_secs(10));

    // Real time: the stub answers the handshake, so the connect succeeds.
    let mut conn = executor
        .get_conn()
        .await
        .expect("handshake completes against the stub");

    tokio::time::pause();

    // With the configured 10s driver deadline governing, PING must still
    // be Pending at the 1s guard boundary — the guard fires. Had the
    // driver's 500ms default governed, the command would have FAILED
    // inside the guard instead.
    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        redis::cmd("PING").query_async::<()>(&mut conn),
    )
    .await;
    let Err(_elapsed) = outcome else {
        panic!(
            "guard must fire: the command must still be Pending at 1s (the driver 500ms default governed?)"
        );
    };
}

#[tokio::test]
async fn configured_small_timeout_fires_before_driver_default() {
    let (addr, _stub) = HandshakeStub::start_silent().expect("bind silent stub");
    let executor = executor_for(addr, Duration::from_millis(100));

    // Real time: the handshake completes.
    let mut conn = executor
        .get_conn()
        .await
        .expect("handshake completes against the stub");

    tokio::time::pause();
    let started = tokio::time::Instant::now();

    let outcome = tokio::time::timeout(
        Duration::from_millis(600),
        redis::cmd("PING").query_async::<()>(&mut conn),
    )
    .await;
    let elapsed = started.elapsed();

    let Ok(query) = outcome else {
        panic!("configured 100ms deadline must fire inside the 600ms guard, not the guard itself");
    };
    assert!(
        query.is_err(),
        "PING against the silent stub must fail with the response-deadline error"
    );
    assert!(
        elapsed >= Duration::from_millis(100) && elapsed < Duration::from_millis(500),
        "deadline must fire in [100ms, 500ms); a regression to the 500ms driver default \
         lands exactly at 500ms and fails this bound; elapsed: {elapsed:?}"
    );
}

#[tokio::test]
async fn refresh_rebuild_carries_configured_deadline() {
    let (addr, _stub) = HandshakeStub::start_silent().expect("bind silent stub");
    let executor = executor_for(addr, Duration::from_millis(100));

    // Real time: the initial connect and the refresh rebuild both
    // complete against the stub (the accept loop serves per-connection).
    assert!(executor.get_conn().await.is_ok());
    let mut conn = executor
        .refresh()
        .await
        .expect("refresh rebuilds a live connection");

    tokio::time::pause();
    let started = tokio::time::Instant::now();

    let outcome = tokio::time::timeout(
        Duration::from_millis(600),
        redis::cmd("PING").query_async::<()>(&mut conn),
    )
    .await;
    let elapsed = started.elapsed();

    let Ok(query) = outcome else {
        panic!(
            "rebuilt connection must inherit the 100ms deadline and fail inside the 600ms guard"
        );
    };
    assert!(
        query.is_err(),
        "PING on the rebuilt connection must fail with the response-deadline error"
    );
    assert!(
        elapsed >= Duration::from_millis(100) && elapsed < Duration::from_millis(500),
        "rebuilt connection deadline must fire in [100ms, 500ms); a rebuild that drops \
         the config regresses to the 500ms driver default; elapsed: {elapsed:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn response_timeout_does_not_alter_connect_timeout() {
    let addr = never_handshake_peer();
    let mut config = RedisEndpointConfig::from_uri(&format!("redis://{addr}"))
        .expect("valid standalone URI for the peer");
    config.connection_timeout_secs = 1;
    let topology = topology_from_config(&config).expect("standalone topology");
    let executor = MultiplexedExecutor::new(config, topology)
        .with_response_timeout(Duration::from_millis(100));

    // The peer accepts TCP but never answers the handshake, and no real
    // I/O ever completes, so paused virtual time jumps deterministically
    // to the component's 1s connect-timeout timer. The response-timeout
    // builder must not shift this bound: the 100ms response deadline
    // governs command awaits, not the connect wrapper.
    let err = executor
        .get_conn()
        .await
        .expect_err("handshake never completes");
    match err {
        CamelError::ProcessorError(msg) => assert!(
            msg.contains("timed out after 1s"),
            "component connect-timeout message must govern, got: {msg}"
        ),
        other => panic!("expected ProcessorError, got: {other:?}"),
    }
}
