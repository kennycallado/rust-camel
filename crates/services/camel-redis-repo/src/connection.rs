//! Eager connection construction for Redis-backed repositories.
//!
//! One topology resolution per repository construction: the returned
//! executor caches the healthy connection, so later `execute` calls never
//! re-resolve; sentinel failover is detected only through explicit
//! `refresh`. Cluster topologies are rejected before any resolution because
//! the repositories assume single-key routing.

use crate::executor::MultiplexedRepoExecutor;
use crate::executor::RepoCommandExecutor;
use camel_api::CamelError;
use camel_component_redis::MultiplexedExecutor;
use camel_component_redis::RedisEndpointConfig;
use camel_component_redis::RedisTopology;
#[cfg(feature = "cluster")]
use camel_component_redis::TopologyKind;
use camel_component_redis::topology_from_config;
use std::sync::Arc;

/// Build the production executor for `endpoint`.
///
/// The topology is derived from the endpoint configuration; construction
/// connects eagerly and fails fast when Redis is unreachable.
pub(crate) async fn connect_executor(
    endpoint: &RedisEndpointConfig,
) -> Result<MultiplexedRepoExecutor, CamelError> {
    // Cluster rejection must precede topology_from_config, whose own
    // cluster branch would surface a component-level message instead.
    #[cfg(feature = "cluster")]
    reject_cluster(endpoint)?;
    let topology = topology_from_config(endpoint)?;
    connect_executor_with_topology(endpoint, topology).await
}

/// Test seam: build the executor against an injected topology.
///
/// Shares the body of [`connect_executor`]: cluster rejection, executor
/// construction, and one eager connect.
pub(crate) async fn connect_executor_with_topology(
    endpoint: &RedisEndpointConfig,
    topology: Arc<dyn RedisTopology>,
) -> Result<MultiplexedRepoExecutor, CamelError> {
    #[cfg(feature = "cluster")]
    reject_cluster(endpoint)?;

    let executor =
        MultiplexedRepoExecutor::new(MultiplexedExecutor::new(endpoint.clone(), topology));

    // Eager connect: fail construction fast when Redis is unreachable. The
    // wrapper remaps the component's ProcessorError connection failures to
    // Io, matching the repository transport contract. No spawn_blocking —
    // RedisTopology::resolve offloads internally where it must.
    executor.refresh().await?;

    Ok(executor)
}

/// Reject cluster topologies before any topology resolution.
///
/// `TopologyKind::Cluster` only exists under the component's `cluster`
/// feature; without it a cluster-shaped config already fails closed during
/// endpoint parsing.
#[cfg(feature = "cluster")]
fn reject_cluster(endpoint: &RedisEndpointConfig) -> Result<(), CamelError> {
    if matches!(endpoint.topology_kind, TopologyKind::Cluster) {
        return Err(CamelError::Config(
            "cluster topology is not supported for repository backends".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::connect_executor;
    use super::connect_executor_with_topology;
    use crate::executor::FakeRedisServer;
    use crate::executor::FakeStaticTopology;
    use crate::executor::RepoCommandExecutor;
    use camel_api::CamelError;
    use camel_component_redis::RedisEndpointConfig;
    use std::sync::Arc;

    /// Standalone config with a short connect timeout so dead-address tests
    /// fail fast.
    fn short_timeout_config() -> RedisEndpointConfig {
        let mut config =
            RedisEndpointConfig::from_uri("redis://localhost:6379").expect("valid standalone URI");
        config.connection_timeout_secs = 1;
        config
    }

    /// Build an owned single-argument command (`Cmd::arg` borrows, so a
    /// chain cannot be passed by value).
    fn cmd(name: &str) -> redis::Cmd {
        let mut cmd = redis::Cmd::new();
        cmd.arg(name);
        cmd
    }

    #[tokio::test]
    async fn connect_executor_eager_connect_fails_fast_once() {
        // Client::open only parses the URL; the TCP connection to port 1 is
        // refused deterministically at construction time.
        let topology = FakeStaticTopology::with_client(
            redis::Client::open("redis://127.0.0.1:1/0").expect("client opens without network"),
        );

        let result =
            connect_executor_with_topology(&short_timeout_config(), Arc::new(topology.clone()))
                .await;

        match result {
            Err(CamelError::Io(_)) => {}
            Err(other) => panic!("expected CamelError::Io, got: {other}"),
            Ok(_) => panic!("expected failure, got Ok"),
        }
        assert_eq!(
            topology.resolve_count(),
            1,
            "construction must attempt exactly one resolve and fail fast"
        );
    }

    #[tokio::test]
    async fn connect_executor_resolves_once_on_healthy_connection() {
        let (addr, _server) = FakeRedisServer::start()
            .await
            .expect("stub server binds an ephemeral loopback port");
        let topology = FakeStaticTopology::with_client(
            redis::Client::open(format!("redis://{addr}/0")).expect("client opens without network"),
        );

        let executor =
            connect_executor_with_topology(&short_timeout_config(), Arc::new(topology.clone()))
                .await
                .expect("healthy stub must construct");

        for _ in 0..2 {
            let reply = executor
                .execute(cmd("PING"))
                .await
                .expect("cached connection serves PING");
            assert!(
                matches!(reply, redis::Value::SimpleString(ref s) if s == "PONG"),
                "expected +PONG, got: {reply:?}"
            );
        }
        assert_eq!(
            topology.resolve_count(),
            1,
            "the healthy connection is cached — execute must not re-resolve"
        );
    }

    // Exercises the production constructor (topology_from_config path)
    // against the stub so the default-feature test build covers it.
    #[tokio::test]
    async fn connect_executor_production_path_against_stub() {
        let (addr, _server) = FakeRedisServer::start()
            .await
            .expect("stub server binds an ephemeral loopback port");
        let endpoint = RedisEndpointConfig::from_uri(&format!("redis://{addr}"))
            .expect("stub address parses as a standalone URI");

        let executor = connect_executor(&endpoint)
            .await
            .expect("production path must construct against the stub");
        let reply = executor
            .execute(cmd("PING"))
            .await
            .expect("connected executor serves PING");
        assert!(matches!(reply, redis::Value::SimpleString(ref s) if s == "PONG"));
    }

    // Design Phase-1 exit criterion: single AND sentinel topology
    // construction. `topology_from_config` is construction-only — it builds
    // the topology object without resolving a connection, so no sentinel
    // node is ever contacted (RedisTopology::resolve does the network work).
    #[test]
    fn sentinel_topology_unit_construction() {
        let endpoint =
            RedisEndpointConfig::from_uri("redis-sentinel://node1:26379,node2:26379/master/0")
                .expect("multi-node sentinel URI parses");
        let topology = camel_component_redis::topology_from_config(&endpoint)
            .expect("sentinel topology must construct without contacting any node");
        // Sync construction proves the sentinel branch needs no I/O: any
        // network dependency would surface as a timeout, not a return value.
        let _ = topology;
    }

    #[cfg(feature = "cluster")]
    #[tokio::test]
    async fn connect_executor_rejects_cluster() {
        use camel_component_redis::TopologyKind;

        let mut endpoint = short_timeout_config();
        endpoint.topology_kind = TopologyKind::Cluster;
        let topology = FakeStaticTopology::default();

        // Injection seam: rejection must fire BEFORE any topology resolve.
        let result = connect_executor_with_topology(&endpoint, Arc::new(topology.clone())).await;
        match result {
            Err(CamelError::Config(message)) => assert!(
                message.contains("not supported for repository backends"),
                "error must name the repository-specific rejection: {message}"
            ),
            Err(other) => panic!("expected CamelError::Config, got: {other}"),
            Ok(_) => panic!("expected failure, got Ok"),
        }
        assert_eq!(
            topology.resolve_count(),
            0,
            "cluster rejection must fire before any topology resolution"
        );

        // Production constructor: same rejection, before topology_from_config.
        let result = connect_executor(&endpoint).await;
        match result {
            Err(CamelError::Config(message)) => assert!(
                message.contains("not supported for repository backends"),
                "error must name the repository-specific rejection: {message}"
            ),
            Err(other) => panic!("expected CamelError::Config, got: {other}"),
            Ok(_) => panic!("expected failure, got Ok"),
        }
    }
}
