#![allow(dead_code)]

use testcontainers::{ContainerAsync, runners::AsyncRunner};
use testcontainers_modules::redis::Redis;
use tokio::sync::OnceCell;

static REDIS: OnceCell<(ContainerAsync<Redis>, String)> = OnceCell::const_new();

/// Shared Redis container, started once and reused across all tests.
/// Returns the `host:port` (e.g. `127.0.0.1:<mapped>`) the component should
/// connect to.
pub async fn shared_redis() -> &'static str {
    REDIS
        .get_or_init(|| async {
            super::init_tracing();
            super::install_crypto_provider();

            let container = Redis::default()
                .start()
                .await
                .expect("Redis container failed to start");
            let port = container
                .get_host_port_ipv4(6379)
                .await
                .expect("Redis port not available");
            let conn_str = format!("127.0.0.1:{port}");
            eprintln!("Redis ready at: {conn_str}");
            (container, conn_str)
        })
        .await
        .1
        .as_str()
}
