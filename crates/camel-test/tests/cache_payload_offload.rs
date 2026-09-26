//! Integration tests for the redis cache repository with disk payload
//! offload (`cache_repo.payload = "disk"`) wired through `Camel.toml`
//! configuration.
//!
//! Every test builds the context the way `camel run` does — a
//! `[default.cache_repo]` TOML block (`backend = "redis"`, `payload =
//! "disk"`, `payload_dir` = a per-test tempdir) loaded through
//! `CamelConfig::from_file` and `configure_context` — then resolves the
//! registered `"redis"` repository. This exercises the real
//! `context_ext.rs` redis wiring arm (`wrap_disk_offload`), not a
//! manually constructed decorator.
//!
//! Covered behaviors: set/get round-trip with the payload rehydrated
//! from the single blob file while the redis index row keeps an empty
//! `bytes` array and a `payload_path`; the startup portability WARN
//! naming the payload directory; a vanished blob degrading `get` and
//! `peek_stale` to misses; the background sweeper reclaiming a dead
//! blob at its filename-encoded death epoch; and `payload_max_ttl`
//! fabricating an expiry for a no-ttl entry.
//!
//! Sweeper timing: blob death epochs are truncated to whole seconds in
//! the file name, so worst-case reclamation is `ttl + stale_retention +
//! sweep_interval` plus up to one second of truncation plus one sweep
//! tick. The sweeper tests therefore poll with `wait::wait_until`
//! (8s budget) instead of a fixed sleep — a bare 500ms sleep would
//! flake whenever the write lands early in a wall-clock second. The
//! sweep interval cannot shrink below 1s (validation floor), so the
//! budgets must absorb the full second-scale worst case.
//!
//! The redis payload tier (`payload = "redis"`) is covered at two
//! layers. Decorator-level tests build the index and its
//! `RedisPayloadStore` over one connection via
//! `connect_with_payload_store` and wrap them in `OffloadRepository`:
//! round-trip with the payload under its own key while the index row
//! stays bytes-empty with a `payload_path`, the finite EXAT deadline on
//! the payload key, and eager predecessor reclaim on overwrite. The
//! boot-path test boots `payload = "redis"` through `Camel.toml` — the
//! `context_ext` redis arm → `connect_with_payload_store` →
//! `wrap_payload_offload` wiring — and round-trips through the
//! registered repository.
//!
//! Each test provisions its own Redis container so keyspaces and blob
//! directories stay isolated.
//!
//! **Requires Docker to be running.** Tests will fail if Docker is unavailable.
//!
//! **Requires `integration-tests` feature to compile and run.**

#![cfg(feature = "integration-tests")]

mod support;
use support::install_crypto_provider;

use camel_api::ComponentMetrics;
use camel_api::cache::{CacheEntry, CacheRepository, ContentType};
use camel_api::metrics::MetricsHandle;
use camel_config::CamelConfig;
use camel_core::cache::OffloadRepository;
use camel_redis_repo::{RedisCacheRepository, RedisEndpointConfig};
use redis::AsyncCommands;
use std::path::Path;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use testcontainers::ContainerAsync;
use testcontainers::GenericImage;
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

// ===========================================================================
// Config-driven context construction (TOML -> from_file -> configure_context)
// ===========================================================================

/// A `[default.cache_repo]` TOML block selecting the redis backend at
/// `url` with `payload = "disk"` offloading into `payload_dir`.
/// `extra` carries the optional offload timing fields
/// (`payload_sweep_interval`, `payload_max_ttl`) as raw TOML lines.
fn disk_offload_toml(url: &str, payload_dir: &str, stale_retention: &str, extra: &str) -> String {
    format!(
        r#"
[default.cache_repo]
backend = "redis"
url = "{url}"
payload = "disk"
payload_dir = "{payload_dir}"
stale_retention = "{stale_retention}"
{extra}
"#
    )
}

/// A `[default.cache_repo]` TOML block selecting the redis backend at
/// `url` with `payload = "redis"` offloading into the repository's own
/// keyspace. No `payload_dir`: validation rejects one on the redis
/// tier. `extra` carries the optional offload timing fields
/// (`payload_sweep_interval`, `payload_max_ttl`) as raw TOML lines.
fn redis_offload_toml(url: &str, stale_retention: &str, extra: &str) -> String {
    format!(
        r#"
[default.cache_repo]
backend = "redis"
url = "{url}"
payload = "redis"
stale_retention = "{stale_retention}"
{extra}
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

/// Process-wide buffer the shared log-capture subscriber writes into.
///
/// Installed exactly once, by whichever test builds a context first.
/// Installing before any `configure_context` call matters twice over:
/// the portability WARN fires during repository wiring, which precedes
/// `configure_context`'s own subscriber init — and that init's
/// `try_init` failure is tolerated (it only WARNs), so the context
/// still builds. Without a pre-installed capture subscriber the WARN
/// would be a no-op (no global subscriber exists in a bare test
/// process), and asserting it would be impossible.
fn shared_log_buffer() -> Arc<Mutex<Vec<u8>>> {
    /// `io::Write` adapter accumulating formatted log records.
    struct SharedWriter(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for SharedWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    static BUFFER: Mutex<Option<Arc<Mutex<Vec<u8>>>>> = Mutex::new(None);
    let mut slot = BUFFER.lock().unwrap();
    if let Some(buf) = slot.as_ref() {
        return Arc::clone(buf);
    }
    let buf: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let writer = Arc::clone(&buf);
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new("camel_config=warn"))
        .with_ansi(false)
        .with_writer(move || SharedWriter(Arc::clone(&writer)))
        .try_init();
    *slot = Some(Arc::clone(&buf));
    Arc::clone(&buf)
}

/// Write the redis disk-offload `cache_repo` config and build the context
/// (eager redis connect and disk-offload wrapping included). `dir` must
/// stay alive for the caller's test duration; keep the `TempDir` binding.
async fn context_with_disk_offload(
    url: &str,
    dir: &Path,
    stale_retention: &str,
    extra: &str,
) -> camel_core::CamelContext {
    install_crypto_provider();
    // Must precede configure_context: see `shared_log_buffer`.
    shared_log_buffer();
    let cfg = load_camel_toml(&disk_offload_toml(
        url,
        dir.to_str().unwrap(),
        stale_retention,
        extra,
    ));
    CamelConfig::configure_context(&cfg)
        .await
        .expect("context builds with redis disk-offload cache_repo")
}

async fn raw_connection(url: &str) -> redis::aio::MultiplexedConnection {
    let client = redis::Client::open(url.to_string()).expect("raw client opens");
    client
        .get_multiplexed_async_connection()
        .await
        .expect("raw connection established")
}

/// Write the redis payload-tier `cache_repo` config and build the context
/// (eager redis connect and payload-store-paired wrapping included). This
/// covers the `context_ext` redis arm → `connect_with_payload_store` →
/// `wrap_payload_offload` wiring end-to-end.
async fn context_with_redis_offload(
    url: &str,
    stale_retention: &str,
    extra: &str,
) -> camel_core::CamelContext {
    install_crypto_provider();
    // Must precede configure_context: see `shared_log_buffer`.
    shared_log_buffer();
    let cfg = load_camel_toml(&redis_offload_toml(url, stale_retention, extra));
    CamelConfig::configure_context(&cfg)
        .await
        .expect("context builds with redis payload-tier cache_repo")
}

/// Decorator-level harness: index and payload store over ONE connection
/// (`connect_with_payload_store`), wrapped in `OffloadRepository` — the
/// same name/`key_prefix` the config path resolves, the suite's 30s
/// retention, and 60s decorator intervals (sweep cadence, fabricated
/// max TTL).
async fn direct_redis_offload_repo(url: &str) -> OffloadRepository {
    install_crypto_provider();
    let (repo, store) = RedisCacheRepository::connect_with_payload_store(
        "redis",
        // Direct `redis://` endpoint config, bypassing Camel.toml.
        &RedisEndpointConfig::from_uri(url).expect("redis:// endpoint parses"),
        "camel:cache",
        Duration::from_secs(30),
        // Live suite: lever-off metrics facade, compile-only wiring.
        ComponentMetrics::new(Arc::new(MetricsHandle::new()), false),
    )
    .await
    .expect("redis cache repository connects with its payload store");
    OffloadRepository::new(
        Arc::new(repo),
        Arc::new(store),
        Duration::from_secs(30),
        Duration::from_secs(60),
        Duration::from_secs(60),
    )
}

/// The raw index row for `key` under the suite's namespace
/// (`{prefix}:{name}:{key}`), decoded as `CacheEntry` JSON.
async fn raw_index_row(conn: &mut redis::aio::MultiplexedConnection, key: &str) -> CacheEntry {
    let raw: String = conn
        .get(format!("camel:cache:redis:{key}"))
        .await
        .expect("raw GET returns the index row");
    serde_json::from_str(&raw).expect("index row is CacheEntry JSON")
}

// ===========================================================================
// Blob-dir helpers
// ===========================================================================

/// Names of the `.blob` files currently present under `dir`.
fn blob_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("payload dir is readable")
        .map(|entry| {
            entry
                .expect("dir entry readable")
                .file_name()
                .to_string_lossy()
                .to_string()
        })
        .filter(|name| name.ends_with(".blob"))
        .collect()
}

/// The single `.blob` file present under `dir`; fails the test when the
/// count is not exactly one.
fn the_blob(dir: &Path) -> std::path::PathBuf {
    let blobs = blob_names(dir);
    assert_eq!(blobs.len(), 1, "expected exactly one blob, got {blobs:?}");
    dir.join(&blobs[0])
}

fn cache_entry(bytes: Vec<u8>) -> CacheEntry {
    CacheEntry {
        bytes,
        payload_path: None,
        content_type: ContentType::Bytes,
        expires_at: None,
    }
}

// ===========================================================================
// Round-trip: blob on disk, empty bytes in the index row, startup WARN
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_disk_offload_round_trip() {
    let (_container, url) = own_redis().await;
    let dir = tempfile::TempDir::new().expect("payload dir tempdir");
    let ctx = context_with_disk_offload(&url, dir.path(), "30s", "").await;

    let repo = ctx
        .cache_repository("redis")
        .expect("redis cache repository registered when payload = disk");
    assert_eq!(repo.name(), "redis");

    // 50KiB payload — large enough that offloading it matters.
    let payload = vec![0xD4u8; 50 * 1024];
    repo.set(
        "k",
        cache_entry(payload.clone()),
        Some(Duration::from_secs(60)),
    )
    .await
    .expect("set succeeds");

    // Exactly one blob file carries the payload.
    let blobs = blob_names(dir.path());
    assert_eq!(
        blobs.len(),
        1,
        "exactly one offloaded blob in the payload dir"
    );

    // Hydrated get returns the identical bytes.
    let got = repo
        .get("k")
        .await
        .expect("get succeeds")
        .expect("entry is present");
    assert_eq!(
        got.bytes, payload,
        "hydrated payload must equal the stored one"
    );

    // The startup portability WARN names the configured payload dir.
    let logs = shared_log_buffer().lock().unwrap().clone();
    let logs = String::from_utf8_lossy(&logs);
    assert!(
        logs.contains("offloaded entries under"),
        "startup WARN must mention offloaded entries, got: {logs}"
    );
    assert!(
        logs.contains(dir.path().to_str().unwrap()),
        "startup WARN must name the payload dir, got: {logs}"
    );

    // The raw index row in redis keeps an empty bytes array and points
    // at the blob on disk (second connection, same pattern as the
    // redis_repositories suite).
    let mut conn = raw_connection(&url).await;
    let raw: String = conn
        .get("camel:cache:redis:k")
        .await
        .expect("raw GET returns the index row");
    let row: serde_json::Value = serde_json::from_str(&raw).expect("index row is JSON");
    assert!(
        row["bytes"].as_array().is_some_and(Vec::is_empty),
        "index row must store an empty bytes array, got: {raw}"
    );
    let stored = row["payload_path"]
        .as_str()
        .expect("index row must carry payload_path");
    assert_eq!(
        stored, blobs[0],
        "index row must name the blob present in the payload dir"
    );
}

// ===========================================================================
// Vanished blob degrades get and peek_stale to misses
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_disk_offload_early_sweep_is_miss() {
    let (_container, url) = own_redis().await;
    let dir = tempfile::TempDir::new().expect("payload dir tempdir");
    let ctx = context_with_disk_offload(&url, dir.path(), "30s", "").await;
    let repo = ctx
        .cache_repository("redis")
        .expect("redis cache repository registered");

    // ttl 60s + retention 30s: the index row stays readable while the
    // blob file is deleted underneath it — the "early sweep" state.
    repo.set(
        "k",
        cache_entry(b"early-sweep-payload".to_vec()),
        Some(Duration::from_secs(60)),
    )
    .await
    .expect("set succeeds");
    let blob = the_blob(dir.path());
    std::fs::remove_file(&blob).expect("blob file deleted");

    let got = repo.get("k").await.expect("get succeeds");
    assert!(got.is_none(), "vanished blob must degrade get to a miss");
    let stale = repo.peek_stale("k").await.expect("peek_stale succeeds");
    assert!(
        stale.is_none(),
        "vanished blob must degrade peek_stale to a miss"
    );
}

// ===========================================================================
// Background sweeper reclaims a dead blob
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_disk_offload_sweeper_reclaims_orphan() {
    let (_container, url) = own_redis().await;
    let dir = tempfile::TempDir::new().expect("payload dir tempdir");
    let ctx =
        context_with_disk_offload(&url, dir.path(), "1ms", "payload_sweep_interval = \"1s\"").await;
    let repo = ctx
        .cache_repository("redis")
        .expect("redis cache repository registered");

    // ttl 100ms + retention 1ms + sweep 1s (the validation floor —
    // sub-second intervals are rejected): the entry dies almost
    // immediately, orphaning the blob for the sweeper to reclaim.
    repo.set(
        "k",
        cache_entry(b"sweeper-orphan".to_vec()),
        Some(Duration::from_millis(100)),
    )
    .await
    .expect("set succeeds");
    let blob = the_blob(dir.path());

    // Death epochs truncate to whole seconds and the sweep ticks every
    // 1s (worst case ~3.1s here), so poll with a generous budget
    // instead of a fixed sleep.
    support::wait::wait_until(
        "sweeper reclaims the dead payload blob",
        Duration::from_secs(8),
        Duration::from_millis(100),
        || async { Ok(!blob.exists()) },
    )
    .await
    .expect("blob must be gone from the payload dir");
}

// ===========================================================================
// payload_max_ttl fabricates an expiry for a no-ttl entry
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_disk_offload_no_ttl_capped() {
    let (_container, url) = own_redis().await;
    let dir = tempfile::TempDir::new().expect("payload dir tempdir");
    let ctx = context_with_disk_offload(
        &url,
        dir.path(),
        "1ms",
        "payload_sweep_interval = \"1s\"\npayload_max_ttl = \"1s\"",
    )
    .await;
    let repo = ctx
        .cache_repository("redis")
        .expect("redis cache repository registered");

    repo.set("k", cache_entry(b"capped-payload".to_vec()), None)
        .await
        .expect("set without ttl succeeds");
    let blob = the_blob(dir.path());

    // The fabricated 1s expiry (the validation floor) has passed (redis
    // EXAT dropped the row at ~ttl + retention): poll for the miss —
    // a fixed sleep would have to out-live the full second and still
    // race the truncation.
    support::wait::wait_until(
        "no-ttl entry expires at payload_max_ttl",
        Duration::from_secs(5),
        Duration::from_millis(100),
        || async {
            repo.get("k")
                .await
                .map(|entry| entry.is_none())
                .map_err(|e| e.to_string())
        },
    )
    .await
    .expect("no-ttl entry must expire at payload_max_ttl");

    // The dead blob is swept from the payload dir.
    support::wait::wait_until(
        "sweeper reclaims the capped payload blob",
        Duration::from_secs(8),
        Duration::from_millis(100),
        || async { Ok(!blob.exists()) },
    )
    .await
    .expect("blob must be swept from the payload dir");
}

// ===========================================================================
// Redis payload tier: index/payload key split with a native EXAT deadline
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_payload_tier_roundtrip_index_split_and_ttl() {
    let (_container, url) = own_redis().await;
    let repo = direct_redis_offload_repo(&url).await;

    repo.set(
        "k",
        cache_entry(b"hello-tier".to_vec()),
        Some(Duration::from_secs(60)),
    )
    .await
    .expect("set succeeds");

    // Hydrated get returns the stored bytes.
    let got = repo
        .get("k")
        .await
        .expect("get succeeds")
        .expect("entry is present");
    assert_eq!(
        got.bytes,
        b"hello-tier".to_vec(),
        "hydrated payload must equal the stored one"
    );

    // The raw index row keeps an empty bytes array and points at the
    // payload key inside the same namespace.
    let mut conn = raw_connection(&url).await;
    let row = raw_index_row(&mut conn, "k").await;
    assert!(
        row.bytes.is_empty(),
        "index row must store no bytes, got {}",
        row.bytes.len()
    );
    let payload_path = row.payload_path.expect("index row must carry payload_path");

    // The payload lives under its own key, bytes verbatim.
    let payload_key = format!("camel:cache:redis:payload:{payload_path}");
    let exists: i64 = conn
        .exists(&payload_key)
        .await
        .expect("payload EXISTS reads");
    assert_eq!(exists, 1, "payload key must exist");
    let stored: Vec<u8> = conn
        .get(&payload_key)
        .await
        .expect("raw GET returns the payload key");
    assert_eq!(
        stored,
        b"hello-tier".to_vec(),
        "payload key must hold the raw bytes"
    );

    // The payload dies at its death epoch — ttl 60s + retention 30s +
    // sweep 60s ≈ 150s — via native EXAT: TTL is live (> 0, never the
    // -1 no-expiry sentinel) and bounded.
    let ttl: i64 = conn.ttl(&payload_key).await.expect("payload TTL reads");
    assert!(
        ttl > 0,
        "payload key must have a live deadline, got TTL {ttl}"
    );
    assert!(
        ttl <= 160,
        "payload deadline must be the finite death epoch (~150s), got TTL {ttl}"
    );
}

// ===========================================================================
// Redis payload tier: overwrite reclaims the predecessor payload key
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_payload_tier_overwrite_reclaims_predecessor() {
    let (_container, url) = own_redis().await;
    let repo = direct_redis_offload_repo(&url).await;
    let mut conn = raw_connection(&url).await;

    repo.set(
        "k",
        cache_entry(b"payload-v1".to_vec()),
        Some(Duration::from_secs(60)),
    )
    .await
    .expect("first set succeeds");
    let predecessor = raw_index_row(&mut conn, "k")
        .await
        .payload_path
        .expect("v1 index row carries payload_path");

    repo.set(
        "k",
        cache_entry(b"payload-v2".to_vec()),
        Some(Duration::from_secs(60)),
    )
    .await
    .expect("second set succeeds");

    // The predecessor payload key is reclaimed eagerly at overwrite.
    let predecessor_key = format!("camel:cache:redis:payload:{predecessor}");
    let exists: i64 = conn
        .exists(&predecessor_key)
        .await
        .expect("predecessor EXISTS reads");
    assert_eq!(
        exists, 0,
        "overwrite must reclaim the predecessor payload key"
    );

    // The successor payload key holds v2 under a fresh name.
    let successor = raw_index_row(&mut conn, "k")
        .await
        .payload_path
        .expect("v2 index row carries payload_path");
    assert_ne!(
        successor, predecessor,
        "overwrite must mint a new payload key"
    );
    let stored: Vec<u8> = conn
        .get(format!("camel:cache:redis:payload:{successor}"))
        .await
        .expect("raw GET returns the successor payload key");
    assert_eq!(
        stored,
        b"payload-v2".to_vec(),
        "successor payload key must hold v2"
    );

    // The hydrated get returns the successor bytes.
    let got = repo
        .get("k")
        .await
        .expect("get succeeds")
        .expect("entry is present");
    assert_eq!(
        got.bytes,
        b"payload-v2".to_vec(),
        "get must return the successor bytes"
    );
}

// ===========================================================================
// Redis payload tier: invalidate_prefix never crosses the payload namespace
// ===========================================================================

/// The `payload:` sub-namespace lives inside the index keyspace, so a user
/// prefix like "p" globs every payload key too ("payload:" itself starts
/// with "p"). The prefix sweep must delete only index rows under the
/// prefix ("p-user" here) while the offloaded payload of "k1" — whose key
/// does NOT start with the prefix — and its blob survive, and the
/// returned count stays index-scoped.
#[tokio::test(flavor = "multi_thread")]
async fn redis_payload_tier_invalidate_prefix_spares_payload_keys() {
    let (_container, url) = own_redis().await;
    let repo = direct_redis_offload_repo(&url).await;
    let mut conn = raw_connection(&url).await;

    repo.set(
        "k1",
        cache_entry(b"tiered-payload".to_vec()),
        Some(Duration::from_secs(60)),
    )
    .await
    .expect("set k1 succeeds");
    // A user key whose name starts with "p": the legit target of
    // invalidate_prefix("p").
    repo.set(
        "p-user",
        cache_entry(b"index-only".to_vec()),
        Some(Duration::from_secs(60)),
    )
    .await
    .expect("set p-user succeeds");

    let removed = repo
        .invalidate_prefix("p")
        .await
        .expect("invalidate_prefix succeeds");
    assert_eq!(
        removed, 1,
        "the count covers only the p-user index row, never payload keys"
    );

    // The "p-user" index row is gone; k1's payload blob survives (raw
    // EXISTS on both shapes).
    let p_row: i64 = conn
        .exists("camel:cache:redis:p-user")
        .await
        .expect("p-user EXISTS reads");
    assert_eq!(p_row, 0, "invalidate_prefix must drop the p-user index row");
    let blob = raw_index_row(&mut conn, "k1")
        .await
        .payload_path
        .expect("k1 index row carries payload_path");
    let payload_exists: i64 = conn
        .exists(format!("camel:cache:redis:payload:{blob}"))
        .await
        .expect("payload EXISTS reads");
    assert_eq!(
        payload_exists, 1,
        "the k1 payload blob must survive the prefix sweep"
    );

    // The surviving row still hydrates its offloaded bytes.
    let got = repo
        .get("k1")
        .await
        .expect("get succeeds")
        .expect("k1 is present");
    assert_eq!(
        got.bytes,
        b"tiered-payload".to_vec(),
        "get must hydrate the surviving payload"
    );
}

// ===========================================================================
// Redis payload tier: boots through Camel.toml (context_ext redis arm)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn redis_payload_tier_boots_from_toml() {
    let (_container, url) = own_redis().await;
    let ctx = context_with_redis_offload(&url, "30s", "").await;

    let repo = ctx
        .cache_repository("redis")
        .expect("redis cache repository registered when payload = redis");
    assert_eq!(repo.name(), "redis");

    repo.set(
        "k",
        cache_entry(b"boot-path-tier".to_vec()),
        Some(Duration::from_secs(60)),
    )
    .await
    .expect("set succeeds");

    let got = repo
        .get("k")
        .await
        .expect("get succeeds")
        .expect("entry is present");
    assert_eq!(
        got.bytes,
        b"boot-path-tier".to_vec(),
        "hydrated payload must equal the stored one"
    );

    // The raw index row stays bytes-empty and points at the payload.
    let mut conn = raw_connection(&url).await;
    let row = raw_index_row(&mut conn, "k").await;
    assert!(
        row.bytes.is_empty(),
        "boot-path index row must store no bytes, got {}",
        row.bytes.len()
    );
    assert!(
        row.payload_path.is_some(),
        "boot-path index row must carry payload_path"
    );
}
