//! Tests for the disk-payload offload decorator, split from
//! `disk_offload.rs` to keep the production file focused.

use super::*;
use crate::cache::MemoryCacheRepository;
use crate::cache::RedbCacheRepository;
use std::collections::HashSet;
use std::path::Path;
use tempfile::tempdir;

/// Standard test tuning: 168h retention, 1h sweep, 24h fabricated TTL.
const RETENTION: Duration = Duration::from_secs(168 * 3600);
const SWEEP: Duration = Duration::from_secs(3600);
const MAX_TTL: Duration = Duration::from_secs(24 * 3600);

fn entry(bytes: Vec<u8>, content_type: ContentType) -> CacheEntry {
    CacheEntry {
        bytes,
        payload_path: None,
        content_type,
        expires_at: None,
    }
}

fn inner_repo() -> Arc<MemoryCacheRepository> {
    Arc::new(MemoryCacheRepository::new("test", 100))
}

fn fixed_clock(at: SystemTime) -> OffloadClock {
    Arc::new(move || at)
}

fn new_repo(
    inner: Arc<MemoryCacheRepository>,
    dir: PathBuf,
    clock: OffloadClock,
) -> DiskOffloadRepository {
    new_repo_dyn(inner, dir, clock)
}

/// [`new_repo`] over any backend (redb stands in for prefix tests).
fn new_repo_dyn(
    inner: Arc<dyn CacheRepository>,
    dir: PathBuf,
    clock: OffloadClock,
) -> DiskOffloadRepository {
    DiskOffloadRepository::with_clock(
        inner,
        dir,
        RETENTION,
        SWEEP,
        MAX_TTL,
        CancellationToken::new(),
        clock,
    )
}

/// All file names currently in `dir`.
fn dir_names(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .expect("read_dir")
        .map(|e| {
            e.expect("dir entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

// ── WARN capture ────────────────────────────────────────────────────────

/// Records the `message` field of each event into a shared buffer.
struct MessageVisitor<'a>(&'a mut String);

impl tracing::field::Visit for MessageVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            *self.0 = format!("{value:?}");
        }
    }
}

/// `tracing_subscriber` layer recording WARN event messages.
struct CaptureLayer {
    events: Arc<Mutex<Vec<String>>>,
}

impl<C> tracing_subscriber::Layer<C> for CaptureLayer
where
    C: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, C>,
    ) {
        if *event.metadata().level() != tracing::Level::WARN {
            return;
        }
        let mut message = String::new();
        event.record(&mut MessageVisitor(&mut message));
        if !message.is_empty() {
            self.events.lock().push(message);
        }
    }
}

/// Install a thread-local subscriber recording WARN messages; returns
/// the shared buffer and the default-subscriber guard.
///
/// Uses `set_default` (guard-scoped, same thread-local mechanism as
/// `tracing::subscriber::with_default`) because a `with_default` sync
/// closure cannot span the `.await` points of an async test body.
fn capture_warns() -> (Arc<Mutex<Vec<String>>>, tracing::subscriber::DefaultGuard) {
    use tracing_subscriber::prelude::*;
    let events = Arc::new(Mutex::new(Vec::new()));
    let layer = CaptureLayer {
        events: Arc::clone(&events),
    };
    let guard = tracing_subscriber::registry().with(layer).set_default();
    (events, guard)
}

// ── set + blob layout ────────────────────────────────────────────────────

#[tokio::test]
async fn set_stores_blob_and_bytes_empty_index_entry() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    let payload: Vec<u8> = (0..50 * 1024).map(|i| (i % 251) as u8).collect();
    repo.set("k", entry(payload.clone(), ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set");

    // Exactly one file in D, with the expected name shape and payload.
    let names = dir_names(dir.path());
    assert_eq!(
        names.len(),
        1,
        "exactly one blob file expected, got {names:?}"
    );
    let name = &names[0];
    let parts: Vec<&str> = name.split('.').collect();
    assert_eq!(parts.len(), 4, "shape hash.epoch.fingerprint.blob: {name}");
    assert_eq!(parts[0], blake3_128hex(b"k"), "key hash component: {name}");
    assert_eq!(
        parts[2],
        content_fingerprint(&entry(payload.clone(), ContentType::Bytes)),
        "fingerprint component: {name}"
    );
    assert_eq!(parts[3], "blob");
    let death = parse_death_epoch(name).expect("death epoch parses");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("post-epoch")
        .as_secs();
    // 1h ttl + 168h retention + 1h sweep = 170h, ±2min slop for the
    // real clock (exactness is pinned by the fixed-clock test below).
    let expected = now + 170 * 3600;
    assert!(
        death.abs_diff(expected) <= 120,
        "death epoch {death} not within 2min of {expected}"
    );
    let on_disk = std::fs::read(dir.path().join(name)).expect("blob read");
    assert_eq!(on_disk, payload, "blob must hold the payload bytes");

    // The index row is stripped: empty bytes + relative payload_path.
    let row = inner
        .peek_stale("k")
        .await
        .expect("peek inner row")
        .expect("inner row present");
    assert!(row.bytes.is_empty(), "index row bytes must be stripped");
    assert_eq!(row.payload_path.as_deref(), Some(name.as_str()));

    // Round-trip through the decorated face, including a second
    // decorator sharing the same dir + inner.
    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, payload);
    assert_eq!(got.content_type, ContentType::Bytes);
    let repo2 = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );
    let got2 = repo2.get("k").await.expect("get2").expect("present2");
    assert_eq!(got2.bytes, payload);
}

#[tokio::test]
async fn death_epoch_formula_uses_ttl_retention_and_grace() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let clock = fixed_clock(UNIX_EPOCH + Duration::from_secs(1_800_000_000));
    let repo = new_repo(Arc::clone(&inner), dir.path().to_path_buf(), clock);

    repo.set(
        "k",
        entry(vec![1, 2, 3], ContentType::Bytes),
        Some(2 * SWEEP),
    )
    .await
    .expect("set");

    let names = dir_names(dir.path());
    assert_eq!(names.len(), 1);
    // C + 2h ttl + 168h retention + 1h sweep, as unix seconds.
    let expected = UNIX_EPOCH
        .checked_add(Duration::from_secs(1_800_000_000))
        .and_then(|c| c.checked_add(2 * SWEEP))
        .and_then(|c| c.checked_add(RETENTION))
        .and_then(|c| c.checked_add(SWEEP))
        .expect("no overflow")
        .duration_since(UNIX_EPOCH)
        .expect("post-epoch")
        .as_secs();
    assert_eq!(
        parse_death_epoch(&names[0]),
        Some(expected),
        "death epoch must be expiry + retention + sweep grace"
    );
}

#[tokio::test]
async fn no_ttl_fabricates_max_ttl_expiry() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let clock = fixed_clock(UNIX_EPOCH + Duration::from_secs(1_800_000_000));
    let repo = new_repo(Arc::clone(&inner), dir.path().to_path_buf(), clock);

    repo.set("k", entry(vec![7; 128], ContentType::Text), None)
        .await
        .expect("set");

    // Blob filename epoch == C + 24h max TTL + retention + sweep.
    let names = dir_names(dir.path());
    assert_eq!(names.len(), 1);
    let expected = UNIX_EPOCH
        .checked_add(Duration::from_secs(1_800_000_000))
        .and_then(|c| c.checked_add(MAX_TTL))
        .and_then(|c| c.checked_add(RETENTION))
        .and_then(|c| c.checked_add(SWEEP))
        .expect("no overflow")
        .duration_since(UNIX_EPOCH)
        .expect("post-epoch")
        .as_secs();
    assert_eq!(
        parse_death_epoch(&names[0]),
        Some(expected),
        "no-ttl set must fabricate the max-TTL death epoch"
    );

    // The inner row's expires_at is recomputed from the inner's own
    // (real) clock: within 2s of real-now + 24h.
    let row = inner
        .peek_stale("k")
        .await
        .expect("peek inner row")
        .expect("inner row present");
    let expires_at = row.expires_at.expect("fabricated expiry must be Some");
    let expected_at = SystemTime::now() + MAX_TTL;
    let skew = if expires_at >= expected_at {
        expires_at.duration_since(expected_at)
    } else {
        expected_at.duration_since(expires_at)
    }
    .expect("comparable times");
    assert!(
        skew <= Duration::from_secs(2),
        "expires_at skew {skew:?} > 2s"
    );
}

#[tokio::test]
async fn blob_write_failure_falls_back_inline() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let (warns, _guard) = capture_warns();
    // `dir` occupied by a regular file: create_dir_all fails.
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"occupied").expect("write blocker");
    let repo = new_repo(Arc::clone(&inner), blocker, default_offload_clock());

    let payload = vec![9; 256];
    repo.set("k", entry(payload.clone(), ContentType::Json), Some(SWEEP))
        .await
        .expect("set must fall back inline, never Err");

    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, payload);
    assert_eq!(got.content_type, ContentType::Json);
    assert!(got.payload_path.is_none(), "fallback row stays inline");
    assert!(
        warns.lock().iter().any(|m| m.contains("blob")),
        "expected a WARN about the failed blob write, got {:?}",
        warns.lock()
    );
}

// ── get / peek_stale re-injection ────────────────────────────────────────

#[tokio::test]
async fn file_dead_read_is_miss_with_warn_never_err() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let (warns, _guard) = capture_warns();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    repo.set("k", entry(vec![5; 64], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set");
    let names = dir_names(dir.path());
    assert_eq!(names.len(), 1);
    std::fs::remove_file(dir.path().join(&names[0])).expect("remove blob");

    assert_eq!(repo.get("k").await.expect("get"), None);
    assert_eq!(repo.peek_stale("k").await.expect("peek_stale"), None);
    assert!(
        warns.lock().iter().any(|m| m.contains("blob")),
        "expected a WARN about the vanished blob, got {:?}",
        warns.lock()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn existing_blob_read_failure_surfaces_err() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );
    repo.set("k", entry(vec![1; 32], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set");
    let names = dir_names(dir.path());
    assert_eq!(names.len(), 1);
    let blob = dir.path().join(&names[0]);

    std::fs::set_permissions(&blob, PermissionsExt::from_mode(0o000)).expect("chmod 000");
    let result = repo.get("k").await;
    // Restore before asserting so TempDir cleanup works even on failure.
    std::fs::set_permissions(&blob, PermissionsExt::from_mode(0o644)).expect("chmod restore");
    assert!(
        result.is_err(),
        "existing-but-unreadable blob must surface Err per Contract C1, got {result:?}"
    );
}

#[tokio::test]
async fn traversal_payload_path_rejected_as_miss() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let (warns, _guard) = capture_warns();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    inner
        .set(
            "a",
            CacheEntry {
                bytes: vec![],
                payload_path: Some("../../etc/passwd".into()),
                content_type: ContentType::Bytes,
                expires_at: None,
            },
            None,
        )
        .await
        .expect("inject row a");
    inner
        .set(
            "b",
            CacheEntry {
                bytes: vec![],
                payload_path: Some("/etc/passwd".into()),
                content_type: ContentType::Bytes,
                expires_at: None,
            },
            None,
        )
        .await
        .expect("inject row b");

    assert_eq!(repo.get("a").await.expect("get a"), None);
    assert_eq!(repo.get("b").await.expect("get b"), None);
    assert_eq!(
        warns.lock().len(),
        2,
        "one WARN per corrupt row, got {:?}",
        warns.lock()
    );
}

#[tokio::test]
async fn legacy_row_without_payload_path_passes_through() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    inner
        .set("k", entry(vec![1, 2, 3], ContentType::Text), None)
        .await
        .expect("inject legacy row");

    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, vec![1, 2, 3]);
    assert_eq!(got.content_type, ContentType::Text);
}

#[tokio::test]
async fn peek_stale_reinjects_past_expiry() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    let payload = vec![3; 32];
    repo.set(
        "k",
        entry(payload.clone(), ContentType::Bytes),
        Some(Duration::from_millis(10)),
    )
    .await
    .expect("set");
    tokio::time::sleep(Duration::from_millis(20)).await;

    let stale = repo
        .peek_stale("k")
        .await
        .expect("peek_stale")
        .expect("stale entry");
    assert_eq!(stale.bytes, payload);
    assert_eq!(repo.get("k").await.expect("get"), None);
}

// ── delete paths & passthrough ──────────────────────────────────────────

#[tokio::test]
async fn invalidate_delegates_and_blob_survives_until_epoch() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    repo.set("k", entry(vec![4; 48], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set");
    assert_eq!(dir_names(dir.path()).len(), 1);

    repo.invalidate("k").await.expect("invalidate");
    assert_eq!(
        repo.get("k").await.expect("get"),
        None,
        "index row must be gone"
    );
    assert_eq!(
        dir_names(dir.path()).len(),
        1,
        "blob must survive invalidation until its death epoch"
    );
}

#[tokio::test]
async fn invalidate_prefix_delegates_count_is_index_scoped() {
    // The ordered redb backend stands in as the index: memory has no
    // key iteration and its invalidate_prefix errs by contract.
    let dir = tempdir().expect("tempdir");
    let inner: Arc<dyn CacheRepository> = Arc::new(
        RedbCacheRepository::new(
            "redb",
            dir.path().join("cache.redb"),
            Duration::from_secs(60),
            None,
            256 * 1024 * 1024,
            Duration::from_secs(3600),
            CancellationToken::new(),
        )
        .await
        .expect("open redb inner"),
    );
    let payload_dir = dir.path().join("payload");
    let repo = new_repo_dyn(inner.clone(), payload_dir.clone(), default_offload_clock());
    // Pins the spec scenario's redb specificity: the decorator's `name()`
    // delegates to the inner backend's registration name.
    assert_eq!(repo.name(), "redb");
    assert_eq!(repo.name(), inner.name());

    for (key, byte) in [("ns:a", b'a'), ("ns:b", b'b'), ("other:c", b'c')] {
        repo.set(key, entry(vec![byte; 32], ContentType::Bytes), Some(SWEEP))
            .await
            .expect("set");
    }
    assert_eq!(dir_names(&payload_dir).len(), 3);

    let deleted = repo
        .invalidate_prefix("ns:")
        .await
        .expect("invalidate_prefix");
    assert_eq!(deleted, 2, "count is index-scoped: only ns: rows removed");
    assert_eq!(
        dir_names(&payload_dir).len(),
        3,
        "all three blobs must remain on disk until their epochs"
    );
}

#[tokio::test]
async fn clear_unlinks_dir_and_delegates() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    repo.set("a", entry(vec![1; 32], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set a");
    repo.set("b", entry(vec![2; 32], ContentType::Text), Some(SWEEP))
        .await
        .expect("set b");
    assert_eq!(dir_names(dir.path()).len(), 2);

    repo.clear().await.expect("clear");

    assert_eq!(inner.stats().await.entries, 0);
    assert!(
        dir_names(dir.path()).is_empty(),
        "payload dir must hold no blob files after clear"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn clear_swallows_unlink_failures() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let (warns, _guard) = capture_warns();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    repo.set("a", entry(vec![1; 16], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set a");
    repo.set("b", entry(vec![2; 16], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set b");
    assert_eq!(dir_names(dir.path()).len(), 2);

    // r-x dir: read_dir succeeds, remove_file is denied per blob.
    std::fs::set_permissions(dir.path(), PermissionsExt::from_mode(0o555)).expect("chmod 555");
    let cleared = repo.clear().await;
    // Restore before asserting so TempDir cleanup works even on failure.
    std::fs::set_permissions(dir.path(), PermissionsExt::from_mode(0o755)).expect("chmod restore");
    assert!(
        cleared.is_ok(),
        "clear must never Err on its own unlink failures: {cleared:?}"
    );
    assert_eq!(inner.stats().await.entries, 0);
    assert!(
        warns.lock().iter().any(|m| m.contains("unlink")),
        "expected WARNs about failed blob unlinks, got {:?}",
        warns.lock()
    );
}

#[tokio::test]
async fn stats_and_name_delegate() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    assert_eq!(repo.name(), inner.name(), "name must pass through");

    repo.set("k", entry(vec![1; 24], ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set");
    repo.get("k").await.expect("get").expect("present");

    let stats = repo.stats().await;
    assert!(
        stats.hits >= 1,
        "hits must flow from inner, got {}",
        stats.hits
    );
    assert_eq!(
        stats.bytes,
        inner.stats().await.bytes,
        "bytes must report the inner value unchanged"
    );
}

// ── fingerprinting ───────────────────────────────────────────────────────

#[tokio::test]
async fn concurrent_same_key_different_payload_no_cross_pair() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let clock = fixed_clock(UNIX_EPOCH + Duration::from_secs(1_800_000_000));
    let repo = new_repo(Arc::clone(&inner), dir.path().to_path_buf(), clock);

    let payload_a = vec![b'A'; 1024];
    let payload_j = vec![b'J'; 2048];
    repo.set("k", entry(payload_a, ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set A");
    repo.set(
        "k",
        entry(payload_j.clone(), ContentType::Json),
        Some(SWEEP),
    )
    .await
    .expect("set J");

    // Two blob files: same key hash + epoch, different fingerprints.
    let names = dir_names(dir.path());
    assert_eq!(names.len(), 2, "two blobs expected, got {names:?}");
    let got = repo.get("k").await.expect("get").expect("present");
    assert_eq!(got.bytes, payload_j, "last writer must win");
    assert_eq!(got.content_type, ContentType::Json);
}

#[tokio::test]
async fn filename_fingerprint_domain_separated() {
    let dir = tempdir().expect("tempdir");
    let inner = inner_repo();
    let repo = new_repo(
        Arc::clone(&inner),
        dir.path().to_path_buf(),
        default_offload_clock(),
    );

    let bytes = vec![42; 100];
    repo.set("a", entry(bytes.clone(), ContentType::Bytes), Some(SWEEP))
        .await
        .expect("set a");
    repo.set("b", entry(bytes.clone(), ContentType::Json), Some(SWEEP))
        .await
        .expect("set b");

    let names = dir_names(dir.path());
    assert_eq!(names.len(), 2, "two blobs expected, got {names:?}");
    let fingerprints: HashSet<&str> = names
        .iter()
        .map(|n| n.split('.').nth(2).expect("fingerprint component"))
        .collect();
    assert_eq!(fingerprints.len(), 2, "content type must separate domains");

    let got_a = repo.get("a").await.expect("get a").expect("present a");
    assert_eq!(got_a.bytes, bytes);
    let got_b = repo.get("b").await.expect("get b").expect("present b");
    assert_eq!(got_b.bytes, bytes);
    assert_eq!(got_b.content_type, ContentType::Json);
}

// ── sweeper ────────────────────────────────────────────────────────────────

#[tokio::test]
async fn sweep_unlinks_dead_and_keeps_live_blobs() {
    let dir = tempdir().expect("tempdir");
    // `now` is far past x's death epoch, far before y's.
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);
    let dead = dir.path().join("x.1799999000.fp.blob");
    let live = dir.path().join("y.1800001000.fp.blob");
    std::fs::write(&dead, b"x").expect("write dead blob");
    std::fs::write(&live, b"y").expect("write live blob");

    let swept = sweep_payload_dir(dir.path(), now, Duration::from_secs(3600)).await;

    assert_eq!(swept, (1, 0), "one dead blob unlinked, nothing else");
    assert!(!dead.exists(), "dead blob must be reclaimed");
    assert!(live.exists(), "live blob must survive the sweep");
}

#[tokio::test]
async fn unlink_payload_file_enoent_is_success() {
    let dir = tempdir().expect("tempdir");
    // A blob path that was never created: it "vanished" between the
    // sweep's listing and its unlink attempt.
    let ghost = dir.path().join("x.1799999000.fp.blob");
    let now = UNIX_EPOCH + Duration::from_secs(1_800_000_000);

    let unlinked = unlink_payload_file(&ghost, now, Duration::from_secs(3600))
        .await
        .expect("ENOENT between listing and unlink is not an error");

    assert!(!unlinked, "vanished blob must report kept, not unlinked");
}

#[tokio::test]
async fn sweep_gcs_stale_tmp_by_age() {
    let dir = tempdir().expect("tempdir");
    let stale = dir.path().join("a.tmp");
    let fresh = dir.path().join("b.tmp");
    std::fs::write(&stale, b"a").expect("write stale tmp");
    std::fs::write(&fresh, b"b").expect("write fresh tmp");
    let two_hours_ago = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("post-epoch")
        .as_secs() as i64
        - 2 * 3600;
    filetime::set_file_mtime(&stale, filetime::FileTime::from_unix_time(two_hours_ago, 0))
        .expect("backdate a.tmp");

    let swept = sweep_payload_dir(dir.path(), SystemTime::now(), Duration::from_secs(3600)).await;

    assert_eq!(swept, (0, 1), "one stale tmp unlinked, nothing else");
    assert!(!stale.exists(), "tmp older than one sweep interval must go");
    assert!(fresh.exists(), "fresh tmp must survive the sweep");
}

#[tokio::test]
async fn sweeper_task_stops_on_shutdown() {
    let dir = tempdir().expect("tempdir");
    let token = CancellationToken::new();
    let handle = spawn_sweeper(
        dir.path().to_path_buf(),
        Duration::from_millis(10),
        token.clone(),
    );

    token.cancel();

    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .expect("sweeper must exit within 2s of shutdown")
        .expect("sweeper task must join cleanly");
}

#[tokio::test]
async fn blob_reclaimed_after_death() {
    let dir = tempdir().expect("tempdir");
    let inner: Arc<dyn CacheRepository> = inner_repo();
    let clock = fixed_clock(UNIX_EPOCH + Duration::from_secs(1_800_000_000));
    // Non-zero cadences only: tokio::time::interval panics on zero, and
    // zero-value validation belongs to CacheRepoConfig, not here.
    let token = CancellationToken::new();
    let repo = DiskOffloadRepository::with_clock(
        inner,
        dir.path().to_path_buf(),
        Duration::ZERO,
        Duration::from_millis(100),
        MAX_TTL,
        token.clone(),
        clock,
    );

    // Isolate the reclaim-under-test from background scheduling: the
    // sweeper's tmp-reap window is one sweep interval (100ms here), and
    // a loaded CI box can stall the tmp→rename leg past it (ENOENT at
    // rename → spurious inline fallback). Stop the sweeper; the direct
    // sweep_payload_dir call below is the behavior under test.
    token.cancel();
    // One yield so the sweeper task observes the cancellation before
    // set() creates its tmp file.
    tokio::time::sleep(Duration::from_millis(10)).await;

    repo.set(
        "k",
        entry(vec![1; 64], ContentType::Bytes),
        Some(Duration::from_millis(100)),
    )
    .await
    .expect("set");

    // Orphan reclaim (invalidate/evict leave blobs behind): sweeping
    // just past the filename-encoded death epoch reclaims the blob
    // without consulting the index.
    let names = dir_names(dir.path());
    assert_eq!(names.len(), 1, "one blob expected, got {names:?}");
    let death = parse_death_epoch(&names[0]).expect("death epoch parses");
    let swept = sweep_payload_dir(
        dir.path(),
        UNIX_EPOCH + Duration::from_secs(death + 1),
        Duration::from_secs(3600),
    )
    .await;
    assert_eq!(swept, (1, 0));
    assert!(
        dir_names(dir.path()).is_empty(),
        "blob must be reclaimed after its death epoch"
    );
}
