//! Compile-only proof that the repository-facing connection seam is
//! reachable from outside the crate (task 1.1, redis-repositories change).

use camel_component_redis::executor::MultiplexedExecutor;
use camel_component_redis::topology_from_config;

#[test]
fn get_conn_and_topology_from_config_are_pub() {
    // Referencing the symbols is the assertion: if visibility regresses,
    // this target stops compiling.
    let _: fn(
        &camel_component_redis::RedisEndpointConfig,
    ) -> Result<
        std::sync::Arc<dyn camel_component_redis::RedisTopology>,
        camel_api::CamelError,
    > = topology_from_config;
    let _ = std::any::type_name::<MultiplexedExecutor>();
    let _: fn(MultiplexedExecutor, std::time::Duration) -> MultiplexedExecutor =
        MultiplexedExecutor::with_response_timeout;
}
