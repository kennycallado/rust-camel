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
//! Each test provisions its own Redis container so keyspaces and blob
//! directories stay isolated.
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
