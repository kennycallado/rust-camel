//! Integration tests for the Redis-backed service repositories
//! (`camel-redis-repo`) wired through `Camel.toml` configuration.
//!
//! Cache section: proves `cache_repo.backend = "redis"` registers a working
//! repository — set/get round-trip with the EXAT stale-retention window,
//! in-band expiry with `peek_stale`, `clear` scoped to the repository prefix,
//! `invalidate_prefix` scoped to one namespace, and registration coexisting
//! with the default `memory` backend.
//!
//! Idempotent section: `add`/`contains`/`remove` semantics over SET NX,
//! `clear` scoped to the `camel:idem` prefix while a cache repository
//! sharing the same Redis database keeps its keys, and registration through
//! `[default.idempotent_repo]` coexisting with the default `memory` backend.
//!
//! Sentinel section: a cache repository selected by `sentinel_nodes` +
//! `master_name` (no `url`) connects to the master the sentinels resolve.
//! A second, authenticated topology proves the `password` field reaches
//! the data nodes: the master runs `requirepass`, the sentinel stays
//! unauthenticated, and a bare client is rejected with NOAUTH.
//!
//! Each cache/idempotent test provisions its own Redis container so
//! keyspaces stay isolated. The sentinel test provisions the shared
//! master + replica + sentinel topology described below.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

mod support;
use support::install_crypto_provider;

use camel_api::cache::{CacheEntry, ContentType};
use camel_config::CamelConfig;
use redis::AsyncCommands;
use std::time::Duration;
use testcontainers::ContainerAsync;
use testcontainers::GenericImage;
use testcontainers::ImageExt;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;

/// Redis image this suite requires. The `testcontainers-modules` default
/// (redis 5.0) predates `SET ... EXAT` (Redis 6.2), which the repository's
/// stale-retention window relies on.
const REDIS_IMAGE_TAG: &str = "7-alpine";

/// Starts a dedicated Redis container for one test. Keep the returned
/// container alive for the duration of the test: dropping it removes it.
async fn own_redis() -> (ContainerAsync<GenericImage>, String) {
    let container = GenericImage::new("redis", REDIS_IMAGE_TAG)
        .with_exposed_port(ContainerPort::Tcp(6379))
        .with_wait_for(WaitFor::message_on_stdout("Ready to accept connections"))
        .start()
        .await
        .expect("Redis container failed to start");
    let port = container
        .get_host_port_ipv4(6379)
        .await
        .expect("Redis port not available");
    (container, format!("redis://127.0.0.1:{port}"))
}

fn cache_entry() -> CacheEntry {
    CacheEntry {
        bytes: b"redis-repo-payload".to_vec(),
        payload_path: None,
        content_type: ContentType::Bytes,
        expires_at: None,
    }
}

/// A `[default.cache_repo]` TOML block selecting the redis backend at `url`.
fn cache_repo_toml(url: &str, stale_retention: &str) -> String {
    format!(
        r#"
[default.cache_repo]
backend = "redis"
url = "{url}"
stale_retention = "{stale_retention}"
"#
    )
}

/// Write `toml_str` into a tempdir `Camel.toml` and load it through the same
/// `CamelConfig::from_file` path `camel run` uses.
fn load_camel_toml(toml_str: &str) -> CamelConfig {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("Camel.toml");
    std::fs::write(&path, toml_str).expect("write Camel.toml");
    CamelConfig::from_file(path.to_str().unwrap()).expect("Camel.toml loads")
}

/// Write the redis `cache_repo` config for `url`, load it, and build the
/// context (eager redis connect included).
async fn context_with_redis_cache(url: &str, stale_retention: &str) -> camel_core::CamelContext {
    install_crypto_provider();
    let cfg = load_camel_toml(&cache_repo_toml(url, stale_retention));
    CamelConfig::configure_context(&cfg)
        .await
        .expect("context builds with redis cache_repo")
}

async fn raw_connection(url: &str) -> redis::aio::MultiplexedConnection {
    let client = redis::Client::open(url.to_string()).expect("raw client opens");
    client
        .get_multiplexed_async_connection()
        .await
        .expect("raw connection established")
}

/// Fallible `raw_connection` for readiness probes: `None` when the server
/// is not accepting connections yet.
async fn try_raw_connection(url: &str) -> Option<redis::aio::MultiplexedConnection> {
    let client = redis::Client::open(url.to_string()).ok()?;
    client.get_multiplexed_async_connection().await.ok()
}

// ===========================================================================
// Registration and round-trip
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn cache_roundtrip_and_ttl() {
    let (_container, url) = own_redis().await;
    let ctx = context_with_redis_cache(&url, "30s").await;

    let repo = ctx
        .cache_repository("redis")
        .expect("redis cache repository registered when backend = redis");
    assert_eq!(repo.name(), "redis");
    assert!(
        ctx.cache_repository("memory").is_some(),
        "default memory cache repository must stay resolvable"
    );

    repo.set("k", cache_entry(), Some(Duration::from_secs(60)))
        .await
        .expect("set succeeds");
    let got = repo.get("k").await.expect("get succeeds");
    let got = got.expect("entry is present before expiry");
    assert_eq!(got.bytes, cache_entry().bytes);
    assert_eq!(got.content_type, ContentType::Bytes);
    assert!(got.expires_at.is_some(), "entry carries expires_at");

    // EXAT = now + 60s ttl + 30s stale retention -> raw TTL ~90s.
    let mut conn = raw_connection(&url).await;
    let ttl: i64 = conn.ttl("camel:cache:redis:k").await.expect("TTL reads");
    assert!(
        ttl > 60 && ttl <= 100,
        "raw TTL must land in (60, 100] seconds, got {ttl}"
    );
}

// ===========================================================================
// In-band expiry and peek_stale
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn cache_in_band_expiry_and_peek_stale_live() {
    let (_container, url) = own_redis().await;
    // Retention (60s) keeps the entry in Redis after in-band expiry.
    let ctx = context_with_redis_cache(&url, "60s").await;
    let repo = ctx
        .cache_repository("redis")
        .expect("redis repo registered");

    repo.set("k", cache_entry(), Some(Duration::from_millis(1)))
        .await
        .expect("set succeeds");
    tokio::time::sleep(Duration::from_millis(20)).await;

    let fresh = repo.get("k").await.expect("get succeeds");
    assert!(fresh.is_none(), "in-band expired entry must read as absent");
    let stale = repo
        .peek_stale("k")
        .await
        .expect("peek_stale succeeds")
        .expect("stale entry is still readable within retention");
    assert_eq!(stale.bytes, cache_entry().bytes);
}

// ===========================================================================
// clear() scoped to the repository prefix
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn cache_clear_scoped_live() {
    let (_container, url) = own_redis().await;
    let ctx = context_with_redis_cache(&url, "30s").await;
    let repo = ctx
        .cache_repository("redis")
        .expect("redis repo registered");

    // Foreign key outside the cache repository prefix must survive clear().
    let mut conn = raw_connection(&url).await;
    let _: () = conn
        .set("camel:idem:x:y", "foreign")
        .await
        .expect("foreign key inserted");

    repo.set("a", cache_entry(), None).await.expect("set a");
    repo.set("b", cache_entry(), None).await.expect("set b");

    repo.clear().await.expect("clear succeeds");

    let a: bool = conn.exists("camel:cache:redis:a").await.expect("EXISTS a");
    let b: bool = conn.exists("camel:cache:redis:b").await.expect("EXISTS b");
    let foreign: bool = conn.exists("camel:idem:x:y").await.expect("EXISTS foreign");
    assert!(!a, "repo key a must be gone after clear");
    assert!(!b, "repo key b must be gone after clear");
    assert!(foreign, "foreign key outside the prefix must survive clear");
}

// ===========================================================================
// invalidate_prefix scoped to one namespace
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn cache_invalidate_prefix_live() {
    let (_container, url) = own_redis().await;
    let ctx = context_with_redis_cache(&url, "30s").await;
    let repo = ctx
        .cache_repository("redis")
        .expect("redis repo registered");

    repo.set("ns:a", cache_entry(), None)
        .await
        .expect("set ns:a");
    repo.set("other:b", cache_entry(), None)
        .await
        .expect("set other:b");

    let removed = repo
        .invalidate_prefix("ns:")
        .await
        .expect("invalidate_prefix succeeds");
    assert_eq!(removed, 1, "exactly one ns: key must be removed");

    let ns_a = repo.get("ns:a").await.expect("get ns:a");
    let other_b = repo.get("other:b").await.expect("get other:b");
    assert!(ns_a.is_none(), "ns:a must be gone");
    assert!(other_b.is_some(), "other:b must survive");
}

// ===========================================================================
// Registration via full CamelConfig build
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn cache_registration_via_config_live() {
    let (_container, url) = own_redis().await;
    let ctx = context_with_redis_cache(&url, "30s").await;

    let repo = ctx
        .cache_repository("redis")
        .expect("redis cache repository resolves via config");
    assert_eq!(repo.name(), "redis");
    assert!(
        ctx.cache_repository("memory").is_some(),
        "memory backend must remain the default alongside redis"
    );
}

// ===========================================================================
// Idempotent repository: add / contains / remove
// ===========================================================================

/// A `[default.idempotent_repo]` TOML block selecting the redis backend at
/// `url`. The default `camel:idem` key prefix applies.
fn idempotent_repo_toml(url: &str) -> String {
    format!(
        r#"
[default.idempotent_repo]
backend = "redis"
url = "{url}"
"#
    )
}

/// Write the redis `idempotent_repo` config for `url`, load it, and build
/// the context (eager redis connect included).
async fn context_with_redis_idempotent(url: &str) -> camel_core::CamelContext {
    install_crypto_provider();
    let cfg = load_camel_toml(&idempotent_repo_toml(url));
    CamelConfig::configure_context(&cfg)
        .await
        .expect("context builds with redis idempotent_repo")
}

#[tokio::test(flavor = "multi_thread")]
async fn idempotent_add_contains_remove_live() {
    let (_container, url) = own_redis().await;
    let ctx = context_with_redis_idempotent(&url).await;

    let repo = ctx
        .idempotent_repository("redis")
        .expect("redis idempotent repository registered when backend = redis");
    assert_eq!(repo.name(), "redis");

    assert!(
        repo.add("m-1").await.expect("first add succeeds"),
        "absent key must report newly added"
    );
    assert!(
        !repo.add("m-1").await.expect("re-add succeeds"),
        "present key must report already present"
    );
    assert!(
        repo.contains("m-1").await.expect("contains succeeds"),
        "added key must be contained"
    );
    repo.remove("m-1").await.expect("remove succeeds");
    assert!(
        !repo
            .contains("m-1")
            .await
            .expect("contains after remove succeeds"),
        "removed key must no longer be contained"
    );
}

// ===========================================================================
// Idempotent clear() scoped to the repository prefix
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn idempotent_clear_scoped_live() {
    let (_container, url) = own_redis().await;

    // Both repositories share one Redis database; their default key
    // prefixes (`camel:cache` vs `camel:idem`) differ, which the
    // cross-repository prefix-collision rule requires.
    let toml = format!(
        r#"
[default.cache_repo]
backend = "redis"
url = "{url}"
stale_retention = "30s"

[default.idempotent_repo]
backend = "redis"
url = "{url}"
"#
    );
    install_crypto_provider();
    let cfg = load_camel_toml(&toml);
    let ctx = CamelConfig::configure_context(&cfg)
        .await
        .expect("context builds with redis cache_repo and idempotent_repo");

    let cache = ctx
        .cache_repository("redis")
        .expect("redis cache repository registered");
    let idem = ctx
        .idempotent_repository("redis")
        .expect("redis idempotent repository registered");

    cache
        .set("shared", cache_entry(), None)
        .await
        .expect("cache set succeeds");
    idem.add("idem-1").await.expect("idempotent add succeeds");

    let mut conn = raw_connection(&url).await;
    let cache_key = "camel:cache:redis:shared";
    let idem_key = "camel:idem:redis:idem-1";
    let cache_before: bool = conn.exists(cache_key).await.expect("EXISTS cache key");
    let idem_before: bool = conn.exists(idem_key).await.expect("EXISTS idem key");
    assert!(
        cache_before,
        "cache key must exist before the idempotent clear"
    );
    assert!(idem_before, "idempotent key must exist before the clear");

    idem.clear().await.expect("idempotent clear succeeds");

    let idem_after: bool = conn.exists(idem_key).await.expect("EXISTS idem key after");
    let cache_after: bool = conn
        .exists(cache_key)
        .await
        .expect("EXISTS cache key after");
    assert!(!idem_after, "idempotent key must be gone after clear");
    assert!(
        cache_after,
        "cache key sharing the database must survive the idempotent clear"
    );
}

// ===========================================================================
// Idempotent registration via full CamelConfig build
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn idempotent_registration_via_config_live() {
    let (_container, url) = own_redis().await;
    let ctx = context_with_redis_idempotent(&url).await;

    let repo = ctx
        .idempotent_repository("redis")
        .expect("redis idempotent repository resolves via config");
    assert_eq!(repo.name(), "redis");
    assert!(
        ctx.idempotent_repository("memory").is_some(),
        "memory backend must remain the default alongside redis"
    );
}

// ===========================================================================
// Sentinel: cache repository selected by sentinel_nodes + master_name
// ===========================================================================

/// Sentinel topology ports for this suite. They differ from the
/// `redis_sentinel_test.rs` ports (16379/16380/26379) so the two test
/// binaries can never collide on the fixed host ports or on each other's
/// stale containers, even when both run in one CI job.
const SENT_MASTER_PORT: u16 = 17379;
const SENT_REPLICA_PORT: u16 = 17380;
const SENT_SENTINEL_PORT: u16 = 27379;
const SENT_MASTER_NAME: &str = "mymaster";

/// Labels this suite's sentinel container so a previous crashed run can be
/// identified and removed before the fixed ports are bound again.
const SENTINEL_LABEL_KEY: &str = "org.rust-camel.redis-repositories-sentinel";
const SENTINEL_LABEL_VALUE: &str = "true";

/// Force-removes sentinel containers left by a previous crashed run of this
/// suite (best-effort, mirroring `redis_sentinel_test.rs`).
async fn remove_stale_sentinel_containers() {
    use std::collections::HashMap;

    let docker = match bollard::Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert(
        "label".to_string(),
        vec![format!("{SENTINEL_LABEL_KEY}={SENTINEL_LABEL_VALUE}")],
    );
    let options = bollard::query_parameters::ListContainersOptionsBuilder::default()
        .all(true)
        .filters(&filters)
        .build();
    let stale = match docker.list_containers(Some(options)).await {
        Ok(list) => list,
        Err(_) => return,
    };
    let remove = bollard::query_parameters::RemoveContainerOptionsBuilder::default()
        .force(true)
        .build();
    for container in stale {
        if let Some(id) = container.id {
            let _ = docker.remove_container(&id, Some(remove.clone())).await;
        }
    }
}

/// Starts the sentinel topology the same way `redis_sentinel_test.rs` does:
/// one container running a master on `SENT_MASTER_PORT`, a replica on
/// `SENT_REPLICA_PORT` announcing itself as `127.0.0.1:SENT_REPLICA_PORT`,
/// and a sentinel on `SENT_SENTINEL_PORT` monitoring `SENT_MASTER_NAME`
/// with quorum 1, each port published with the same number on the host.
/// This suite never triggers a failover, so readiness only waits for the
/// sentinel to track the initial master — the replica-connected gate that
/// failover tests need is unnecessary here.
async fn sentinel_topology() -> ContainerAsync<GenericImage> {
    remove_stale_sentinel_containers().await;

    let script = format!(
        "set -e\n\
         redis-server --port {SENT_MASTER_PORT} --daemonize yes\n\
         until redis-cli -p {SENT_MASTER_PORT} ping | grep -q PONG; do sleep 0.1; done\n\
         redis-server --port {SENT_REPLICA_PORT} --daemonize yes \
         --slaveof 127.0.0.1 {SENT_MASTER_PORT} \
         --slave-announce-ip 127.0.0.1 \
         --slave-announce-port {SENT_REPLICA_PORT}\n\
         until redis-cli -p {SENT_REPLICA_PORT} ping | grep -q PONG; do sleep 0.1; done\n\
         printf 'port {SENT_SENTINEL_PORT}\\n\
         sentinel monitor {SENT_MASTER_NAME} 127.0.0.1 {SENT_MASTER_PORT} 1\\n\
         sentinel down-after-milliseconds {SENT_MASTER_NAME} 2000\\n\
         sentinel failover-timeout {SENT_MASTER_NAME} 10000\\n\
         sentinel parallel-syncs {SENT_MASTER_NAME} 1\\n' > /tmp/sentinel.conf\n\
         exec redis-sentinel /tmp/sentinel.conf\n"
    );

    let image = GenericImage::new("redis", REDIS_IMAGE_TAG)
        .with_cmd(["sh", "-c", &script])
        .with_label(SENTINEL_LABEL_KEY, SENTINEL_LABEL_VALUE)
        .with_mapped_port(SENT_MASTER_PORT, ContainerPort::Tcp(SENT_MASTER_PORT))
        .with_mapped_port(SENT_REPLICA_PORT, ContainerPort::Tcp(SENT_REPLICA_PORT))
        .with_mapped_port(SENT_SENTINEL_PORT, ContainerPort::Tcp(SENT_SENTINEL_PORT))
        .with_ready_conditions(vec![WaitFor::message_on_stdout("+monitor")]);

    image
        .start()
        .await
        .expect("redis sentinel topology failed to start")
}

/// Current master port as tracked by the sentinel, when it reports the
/// expected loopback address.
async fn sentinel_master_port() -> Option<u16> {
    let mut conn = try_raw_connection(&format!("redis://127.0.0.1:{SENT_SENTINEL_PORT}")).await?;
    let (ip, port): (String, String) = redis::cmd("SENTINEL")
        .arg("get-master-addr-by-name")
        .arg(SENT_MASTER_NAME)
        .query_async(&mut conn)
        .await
        .ok()?;
    if ip == "127.0.0.1" {
        port.parse().ok()
    } else {
        None
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cache_sentinel_selected_by_config_live() {
    let _container = sentinel_topology().await;
    support::wait::wait_until(
        "sentinel tracks the master",
        Duration::from_secs(30),
        Duration::from_millis(250),
        || async { Ok(sentinel_master_port().await == Some(SENT_MASTER_PORT)) },
    )
    .await
    .expect("sentinel topology never became ready");

    // `sentinel_nodes` + `master_name` select the topology; no `url` is
    // set. Node entries are bare host:port, as the redis-failover spec
    // scenario writes them.
    let toml = format!(
        r#"
[default.cache_repo]
backend = "redis"
sentinel_nodes = ["127.0.0.1:{SENT_SENTINEL_PORT}"]
master_name = "{SENT_MASTER_NAME}"
stale_retention = "30s"
"#
    );
    install_crypto_provider();
    let cfg = load_camel_toml(&toml);
    let ctx = CamelConfig::configure_context(&cfg)
        .await
        .expect("context builds with sentinel-selected redis cache_repo");

    let repo = ctx
        .cache_repository("redis")
        .expect("redis cache repository resolves via sentinel config");
    assert_eq!(repo.name(), "redis");

    repo.set("k", cache_entry(), None)
        .await
        .expect("set succeeds against the sentinel-resolved master");
    let got = repo
        .get("k")
        .await
        .expect("get succeeds")
        .expect("entry is present on the master");
    assert_eq!(got.bytes, cache_entry().bytes);

    // The write must be visible on the node the sentinels call master —
    // proving the repository connection targeted it, not a fixed address.
    let master_port = sentinel_master_port()
        .await
        .expect("sentinel reports a master after the round-trip");
    let mut master = raw_connection(&format!("redis://127.0.0.1:{master_port}")).await;
    let present: bool = master
        .exists("camel:cache:redis:k")
        .await
        .expect("EXISTS on the master");
    assert!(present, "key must live on the sentinel-resolved master");
}

// ===========================================================================
// Sentinel with data-node authentication: cache_repo password round-trip
// ===========================================================================

/// Ports for the authenticated sentinel topology. They differ from both
/// the unauthenticated `SENT_*` ports above and the `redis_sentinel_test.rs`
/// ports (16379/16380/26379), so no suite can collide on the fixed host
/// ports or on another suite's stale containers.
const SENT_AUTH_MASTER_PORT: u16 = 17389;
const SENT_AUTH_REPLICA_PORT: u16 = 17390;
const SENT_AUTH_SENTINEL_PORT: u16 = 27389;
const SENT_AUTH_MASTER_NAME: &str = "mymaster";

/// Password the authenticated topology's master requires from clients
/// (`requirepass`) and the replica uses for replication (`masterauth`).
/// The sentinel itself stays unauthenticated; it learns the data-node
/// credential through `sentinel auth-pass`.
const SENT_AUTH_PASSWORD: &str = "master-secret";

/// Labels this suite's authenticated sentinel containers so a previous
/// crashed run can be identified and removed before the fixed ports are
/// bound again. Separate from the unauthenticated label so the two
/// topologies' cleanups never remove each other.
const SENT_AUTH_LABEL_KEY: &str = "org.rust-camel.redis-repositories-sentinel-auth";
const SENT_AUTH_LABEL_VALUE: &str = "true";

/// Force-removes authenticated-topology containers left by a previous
/// crashed run of this suite (best-effort, mirroring the unauthenticated
/// variant above).
async fn remove_stale_sentinel_auth_containers() {
    use std::collections::HashMap;

    let docker = match bollard::Docker::connect_with_local_defaults() {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut filters: HashMap<String, Vec<String>> = HashMap::new();
    filters.insert(
        "label".to_string(),
        vec![format!("{SENT_AUTH_LABEL_KEY}={SENT_AUTH_LABEL_VALUE}")],
    );
    let options = bollard::query_parameters::ListContainersOptionsBuilder::default()
        .all(true)
        .filters(&filters)
        .build();
    let stale = match docker.list_containers(Some(options)).await {
        Ok(list) => list,
        Err(_) => return,
    };
    let remove = bollard::query_parameters::RemoveContainerOptionsBuilder::default()
        .force(true)
        .build();
    for container in stale {
        if let Some(id) = container.id {
            let _ = docker.remove_container(&id, Some(remove.clone())).await;
        }
    }
}

/// Starts the authenticated sentinel topology: the same three-role layout
/// as `sentinel_topology` but with the data nodes requiring the password —
/// master `requirepass`, replica `masterauth` — and the sentinel told the
/// credential via `sentinel auth-pass` so it can keep monitoring the
/// master. The sentinel accepts unauthenticated connections.
async fn sentinel_auth_topology() -> ContainerAsync<GenericImage> {
    remove_stale_sentinel_auth_containers().await;

    let script = format!(
        "set -e\n\
         redis-server --port {SENT_AUTH_MASTER_PORT} \
         --requirepass {SENT_AUTH_PASSWORD} --daemonize yes\n\
         until redis-cli --no-auth-warning -a {SENT_AUTH_PASSWORD} \
         -p {SENT_AUTH_MASTER_PORT} ping | grep -q PONG; do sleep 0.1; done\n\
         redis-server --port {SENT_AUTH_REPLICA_PORT} --daemonize yes \
         --slaveof 127.0.0.1 {SENT_AUTH_MASTER_PORT} \
         --masterauth {SENT_AUTH_PASSWORD} \
         --slave-announce-ip 127.0.0.1 \
         --slave-announce-port {SENT_AUTH_REPLICA_PORT}\n\
         until redis-cli -p {SENT_AUTH_REPLICA_PORT} ping | grep -q PONG; do sleep 0.1; done\n\
         printf 'port {SENT_AUTH_SENTINEL_PORT}\\n\
         sentinel monitor {SENT_AUTH_MASTER_NAME} 127.0.0.1 {SENT_AUTH_MASTER_PORT} 1\\n\
         sentinel auth-pass {SENT_AUTH_MASTER_NAME} {SENT_AUTH_PASSWORD}\\n\
         sentinel down-after-milliseconds {SENT_AUTH_MASTER_NAME} 2000\\n\
         sentinel failover-timeout {SENT_AUTH_MASTER_NAME} 10000\\n\
         sentinel parallel-syncs {SENT_AUTH_MASTER_NAME} 1\\n' > /tmp/sentinel.conf\n\
         exec redis-sentinel /tmp/sentinel.conf\n"
    );

    let image = GenericImage::new("redis", REDIS_IMAGE_TAG)
        .with_cmd(["sh", "-c", &script])
        .with_label(SENT_AUTH_LABEL_KEY, SENT_AUTH_LABEL_VALUE)
        .with_mapped_port(
            SENT_AUTH_MASTER_PORT,
            ContainerPort::Tcp(SENT_AUTH_MASTER_PORT),
        )
        .with_mapped_port(
            SENT_AUTH_REPLICA_PORT,
            ContainerPort::Tcp(SENT_AUTH_REPLICA_PORT),
        )
        .with_mapped_port(
            SENT_AUTH_SENTINEL_PORT,
            ContainerPort::Tcp(SENT_AUTH_SENTINEL_PORT),
        )
        .with_ready_conditions(vec![WaitFor::message_on_stdout("+monitor")]);

    image
        .start()
        .await
        .expect("authenticated redis sentinel topology failed to start")
}

/// Current master port as tracked by the authenticated sentinel, when it
/// reports the expected loopback address.
async fn sentinel_auth_master_port() -> Option<u16> {
    let mut conn =
        try_raw_connection(&format!("redis://127.0.0.1:{SENT_AUTH_SENTINEL_PORT}")).await?;
    let (ip, port): (String, String) = redis::cmd("SENTINEL")
        .arg("get-master-addr-by-name")
        .arg(SENT_AUTH_MASTER_NAME)
        .query_async(&mut conn)
        .await
        .ok()?;
    if ip == "127.0.0.1" {
        port.parse().ok()
    } else {
        None
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn cache_sentinel_data_auth_live() {
    let _container = sentinel_auth_topology().await;
    support::wait::wait_until(
        "authenticated sentinel tracks the master",
        Duration::from_secs(30),
        Duration::from_millis(250),
        || async { Ok(sentinel_auth_master_port().await == Some(SENT_AUTH_MASTER_PORT)) },
    )
    .await
    .expect("authenticated sentinel topology never became ready");

    // Same sentinel selection as the unauthenticated test, plus the
    // data-node credential: `password` must reach the master connection
    // that the sentinel resolves.
    let toml = format!(
        r#"
[default.cache_repo]
backend = "redis"
sentinel_nodes = ["127.0.0.1:{SENT_AUTH_SENTINEL_PORT}"]
master_name = "{SENT_AUTH_MASTER_NAME}"
password = "{SENT_AUTH_PASSWORD}"
stale_retention = "30s"
"#
    );
    install_crypto_provider();
    let cfg = load_camel_toml(&toml);
    let ctx = CamelConfig::configure_context(&cfg)
        .await
        .expect("context builds with authenticated sentinel cache_repo");

    let repo = ctx
        .cache_repository("redis")
        .expect("redis cache repository resolves via authenticated sentinel config");
    assert_eq!(repo.name(), "redis");

    repo.set("k", cache_entry(), None)
        .await
        .expect("set succeeds against the authenticated master");
    let got = repo
        .get("k")
        .await
        .expect("get succeeds")
        .expect("entry is present on the authenticated master");
    assert_eq!(got.bytes, cache_entry().bytes);

    // The master must reject a raw client WITHOUT the password — proving
    // it truly enforces auth, so the round-trip above could only have
    // worked by authenticating. With `requirepass`, the rejection surfaces
    // either at the multiplexed handshake or on the first command; PING's
    // plain bulk reply keeps a would-be success unambiguous.
    let bare = redis::Client::open(format!("redis://127.0.0.1:{SENT_AUTH_MASTER_PORT}"))
        .expect("bare client parses");
    let bare_err = match bare.get_multiplexed_async_connection().await {
        Err(err) => err.to_string(),
        Ok(mut conn) => redis::cmd("PING")
            .query_async::<String>(&mut conn)
            .await
            .expect_err("unauthenticated PING must be rejected")
            .to_string(),
    };
    assert!(
        bare_err.contains("NOAUTH") || bare_err.contains("WRONGPASS"),
        "expected NOAUTH/WRONGPASS from the authenticated master, got: {bare_err}"
    );
}
